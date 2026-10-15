//! Read-only Exchange Web Services (EWS) discovery for groupware servers such as grommunio.
//!
//! This module deliberately implements only a folder probe. It does not synchronise items or
//! claim full Exchange compatibility.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use roxmltree::{Document, Node};

use crate::error::{NetError, Result};
use crate::http::{HttpClient, Url};

const MAX_XML: usize = 2 * 1024 * 1024;
const MSG_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/messages";
const TYPE_NS: &str = "http://schemas.microsoft.com/exchange/services/2006/types";
const SOAP_NS: &str = "http://schemas.xmlsoap.org/soap/envelope/";
const SOAP_ACTION: &str = "\"http://schemas.microsoft.com/exchange/services/2006/messages/GetFolder\"";
const FOLDERS: [(&str, &str); 6] = [
    ("inbox", "Mail"),
    ("calendar", "Calendar"),
    ("contacts", "Contacts"),
    ("tasks", "Tasks"),
    ("notes", "Notes"),
    ("msgfolderroot", "Mailbox"),
];

/// The result for a single distinguished folder.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EwsFolder {
    /// EWS distinguished folder id, such as `calendar` or `notes`.
    pub distinguished_id: String,
    /// User-facing category for the discovered folder.
    pub kind: String,
    /// Opaque server folder id on success.
    pub id: Option<String>,
    /// Server error code and message when this folder could not be discovered.
    pub error: Option<String>,
}

/// Performs an authenticated, read-only EWS `GetFolder` probe.
///
/// The URL must be the EWS endpoint. Basic credentials are sent only over HTTPS unless
/// `http.options.allow_plaintext_auth` is enabled by the caller.
pub fn probe(url: &str, username: &str, password: &str, http: &HttpClient) -> Result<Vec<EwsFolder>> {
    let parsed = Url::parse(url)?;
    if username.bytes().any(|b| b == b'\r' || b == b'\n')
        || password.bytes().any(|b| b == b'\r' || b == b'\n')
    {
        return Err(NetError::Invalid("CR/LF in EWS credentials".into()));
    }
    if username.is_empty() {
        return Err(NetError::Invalid("EWS username is empty".into()));
    }
    let auth = STANDARD.encode(format!("{username}:{password}"));
    let auth_header = format!("Basic {auth}");
    let body = request_body();
    let mut bounded_http = http.clone();
    bounded_http.max_body = bounded_http.max_body.min(MAX_XML);
    let response = bounded_http.request(
        "POST",
        &parsed.to_string(),
        &[
            ("Authorization", &auth_header),
            ("Content-Type", "text/xml; charset=utf-8"),
            ("Accept", "text/xml"),
            ("SOAPAction", SOAP_ACTION),
        ],
        Some(body.as_bytes()),
    )?;
    if !response.is_success() {
        if matches!(response.status, 401 | 403) {
            return Err(NetError::Auth(format!("EWS HTTP {}", response.status)));
        }
        return Err(NetError::Http { status: response.status, body: response.text_snippet() });
    }
    parse_get_folder_response(&response.body)
}

fn request_body() -> String {
    let ids =
        FOLDERS.iter().map(|(id, _)| format!("<t:DistinguishedFolderId Id=\"{id}\"/>")).collect::<String>();
    format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?><s:Envelope xmlns:s=\"{SOAP_NS}\" xmlns:m=\"{MSG_NS}\" xmlns:t=\"{TYPE_NS}\"><s:Header><t:RequestServerVersion Version=\"Exchange2013_SP1\"/></s:Header><s:Body><m:GetFolder><m:FolderShape><t:BaseShape>IdOnly</t:BaseShape></m:FolderShape><m:FolderIds>{ids}</m:FolderIds></m:GetFolder></s:Body></s:Envelope>"
    )
}

/// Parses a bounded SOAP response and retains individual folder failures.
fn parse_get_folder_response(xml: &[u8]) -> Result<Vec<EwsFolder>> {
    if xml.len() > MAX_XML {
        return Err(NetError::TooLarge("EWS response exceeds 2 MiB".into()));
    }
    let text =
        std::str::from_utf8(xml).map_err(|_| NetError::Protocol("EWS response is not UTF-8".into()))?;
    let doc = Document::parse(text).map_err(|e| NetError::Protocol(format!("malformed EWS XML: {e}")))?;
    let root = doc.root_element();
    if !is_element(root, "Envelope", SOAP_NS) {
        return Err(NetError::Protocol("EWS response root is not a SOAP Envelope".into()));
    }
    let body = root
        .children()
        .find(|n| is_element(*n, "Body", SOAP_NS))
        .ok_or_else(|| NetError::Protocol("EWS SOAP response has no Body".into()))?;
    if let Some(fault) = body.children().find(|n| is_element(*n, "Fault", SOAP_NS)) {
        let reason = fault
            .descendants()
            .filter(|n| n.is_element())
            .find_map(|n| matches!(n.tag_name().name(), "faultstring" | "Text").then(|| n.text()).flatten())
            .unwrap_or("unspecified SOAP fault");
        return Err(NetError::Refused(format!("EWS SOAP fault: {reason}")));
    }
    let response = body
        .children()
        .find(|n| is_element(*n, "GetFolderResponse", MSG_NS))
        .ok_or_else(|| NetError::Protocol("EWS SOAP Body has no GetFolderResponse".into()))?;
    let messages = response.descendants().filter(|n| is_element(*n, "GetFolderResponseMessage", MSG_NS));
    let mut results = Vec::new();
    for (index, message) in messages.enumerate() {
        let Some((distinguished_id, kind)) = FOLDERS.get(index) else { break };
        let code =
            child_text(message, "ResponseCode", MSG_NS).unwrap_or_else(|| "MissingResponseCode".into());
        let class = message.attribute("ResponseClass").unwrap_or("");
        if code == "NoError" && class == "Success" {
            let id = message
                .descendants()
                .find(|n| is_element(*n, "FolderId", TYPE_NS))
                .and_then(|n| n.attribute("Id"))
                .map(str::to_owned);
            if id.is_none() {
                results.push(folder_result(
                    distinguished_id,
                    kind,
                    None,
                    Some("successful response omitted FolderId".into()),
                ));
            } else {
                results.push(folder_result(distinguished_id, kind, id, None));
            }
        } else {
            let message_text = child_text(message, "MessageText", MSG_NS).unwrap_or_default();
            let detail = if message_text.is_empty() { code } else { format!("{code}: {message_text}") };
            results.push(folder_result(distinguished_id, kind, None, Some(detail)));
        }
    }
    if results.is_empty() {
        return Err(NetError::Protocol("EWS response contains no GetFolderResponseMessage entries".into()));
    }
    while results.len() < FOLDERS.len() {
        let Some((distinguished_id, kind)) = FOLDERS.get(results.len()) else { break };
        results.push(folder_result(
            distinguished_id,
            kind,
            None,
            Some("server omitted this folder response".into()),
        ));
    }
    Ok(results)
}

fn is_element(node: Node<'_, '_>, local: &str, namespace: &str) -> bool {
    node.is_element() && node.tag_name().name() == local && node.tag_name().namespace() == Some(namespace)
}

fn child_text(node: Node<'_, '_>, local: &str, namespace: &str) -> Option<String> {
    node.descendants().find(|n| is_element(*n, local, namespace)).and_then(|n| n.text()).map(str::to_owned)
}

fn folder_result(id: &str, kind: &str, folder_id: Option<String>, error: Option<String>) -> EwsFolder {
    EwsFolder { distinguished_id: id.into(), kind: kind.into(), id: folder_id, error }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn response(codes: &[(&str, &str)]) -> String {
        let messages = codes.iter().enumerate().map(|(i, (code, class))| {
            let folder_id = if *code == "NoError" { format!("<m:Folders><t:Folder><t:FolderId Id=\"f{i}\"/></t:Folder></m:Folders>") } else { String::new() };
            let text = if *code == "NoError" { String::new() } else { "<m:MessageText>Folder unavailable</m:MessageText>".into() };
            format!("<m:GetFolderResponseMessage ResponseClass=\"{class}\"><m:ResponseCode>{code}</m:ResponseCode>{text}{folder_id}</m:GetFolderResponseMessage>")
        }).collect::<String>();
        format!(
            "<s:Envelope xmlns:s=\"{SOAP_NS}\" xmlns:m=\"{MSG_NS}\" xmlns:t=\"{TYPE_NS}\"><s:Body><m:GetFolderResponse><m:ResponseMessages>{messages}</m:ResponseMessages></m:GetFolderResponse></s:Body></s:Envelope>"
        )
    }

    fn serve_once(status: u16, body: Vec<u8>) -> (String, std::thread::JoinHandle<String>) {
        use std::io::{Read, Write};
        use std::net::TcpListener;

        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let server = std::thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            let _ = stream.set_read_timeout(Some(std::time::Duration::from_secs(5)));
            let mut request = Vec::new();
            let mut buffer = [0; 4096];
            let mut header_end = None;
            let mut content_length = 0usize;
            loop {
                let read = stream.read(&mut buffer).unwrap();
                if read == 0 {
                    break;
                }
                request.extend_from_slice(&buffer[..read]);
                if header_end.is_none()
                    && let Some(end) = request.windows(4).position(|window| window == b"\r\n\r\n")
                {
                    header_end = Some(end + 4);
                    let headers = String::from_utf8_lossy(&request[..end]);
                    content_length = headers
                        .lines()
                        .find_map(|line| {
                            let (key, value) = line.split_once(':')?;
                            key.eq_ignore_ascii_case("content-length")
                                .then(|| value.trim().parse().ok())
                                .flatten()
                        })
                        .unwrap_or(0);
                }
                if header_end.is_some_and(|end| request.len() >= end.saturating_add(content_length)) {
                    break;
                }
            }
            let reason = match status {
                200 => "OK",
                401 => "Unauthorized",
                403 => "Forbidden",
                _ => "Synthetic",
            };
            let head = format!(
                "HTTP/1.1 {status} {reason}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(head.as_bytes());
            let _ = stream.write_all(&body);
            String::from_utf8_lossy(&request).into_owned()
        });
        (format!("http://{address}/EWS/Exchange.asmx"), server)
    }

    #[test]
    fn request_is_read_only_and_covers_groupware_folders() {
        let request = request_body();
        assert_eq!(SOAP_ACTION, "\"http://schemas.microsoft.com/exchange/services/2006/messages/GetFolder\"");
        assert!(request.contains("<m:GetFolder>"));
        assert!(!request.contains("CreateItem"));
        assert!(!request.contains("EmailAddress>me"));
        for id in ["inbox", "calendar", "contacts", "tasks", "notes"] {
            assert!(request.contains(&format!("Id=\"{id}\"")));
        }
    }

    #[test]
    fn maps_realistic_success_and_mixed_per_folder_failures() {
        let all_ok = response(&[("NoError", "Success"); 6]);
        let folders = parse_get_folder_response(all_ok.as_bytes()).unwrap();
        assert_eq!(folders.len(), 6);
        assert_eq!(folders[0].id.as_deref(), Some("f0"));
        assert_eq!(folders[1].kind, "Calendar");

        let mixed = response(&[
            ("NoError", "Success"),
            ("ErrorFolderNotFound", "Error"),
            ("NoError", "Success"),
            ("NoError", "Success"),
            ("ErrorAccessDenied", "Error"),
            ("NoError", "Success"),
        ]);
        let folders = parse_get_folder_response(mixed.as_bytes()).unwrap();
        assert_eq!(folders[1].error.as_deref(), Some("ErrorFolderNotFound: Folder unavailable"));
        assert_eq!(folders[2].id.as_deref(), Some("f2"));
        assert!(folders[4].error.is_some());
    }

    #[test]
    fn validates_malformed_xml_faults_and_namespaces() {
        assert!(parse_get_folder_response(b"<bad").is_err());
        let fault = format!(
            "<s:Envelope xmlns:s=\"{SOAP_NS}\"><s:Body><s:Fault><faultstring>denied</faultstring></s:Fault></s:Body></s:Envelope>"
        );
        assert!(matches!(parse_get_folder_response(fault.as_bytes()), Err(NetError::Refused(_))));
        assert!(
            parse_get_folder_response(b"<Envelope><Body><GetFolderResponse/></Body></Envelope>").is_err()
        );
    }

    #[test]
    fn public_probe_sends_post_action_and_basic_auth_when_plaintext_is_explicit() {
        let (url, server) = serve_once(200, response(&[("NoError", "Success"); 6]).into_bytes());
        let http =
            HttpClient::new(crate::ConnectOptions { allow_plaintext_auth: true, ..Default::default() });
        let folders = probe(&url, "alice@example.org", "synthetic-secret", &http).unwrap();
        assert_eq!(folders.len(), 6);
        let request = server.join().unwrap();
        assert!(request.starts_with("POST /EWS/Exchange.asmx HTTP/1.1"));
        assert!(request.contains(&format!("SOAPAction: {SOAP_ACTION}")));
        let authorization = format!("Basic {}", STANDARD.encode("alice@example.org:synthetic-secret"));
        assert!(request.contains(&format!("Authorization: {authorization}")));
        assert!(request.contains("Id=\"calendar\""));
        assert!(!request.contains("EmailAddress>me"));
    }

    #[test]
    fn public_probe_requires_opt_in_for_plaintext_auth() {
        let error = probe("http://127.0.0.1:1/EWS", "alice", "secret", &HttpClient::default()).unwrap_err();
        assert!(matches!(error, NetError::Insecure(_)));
    }

    #[test]
    fn public_probe_maps_unauthorized_statuses_to_auth_errors() {
        for status in [401, 403] {
            let (url, server) = serve_once(status, b"denied".to_vec());
            let http =
                HttpClient::new(crate::ConnectOptions { allow_plaintext_auth: true, ..Default::default() });
            let error = probe(&url, "alice", "secret", &http).unwrap_err();
            assert!(matches!(error, NetError::Auth(_)), "status {status}: {error:?}");
            assert!(server.join().unwrap().contains("Authorization: Basic "));
        }
    }

    #[test]
    fn public_probe_caps_http_response_body() {
        let (url, server) = serve_once(200, vec![b'x'; MAX_XML + 1]);
        let http =
            HttpClient::new(crate::ConnectOptions { allow_plaintext_auth: true, ..Default::default() });
        let error = probe(&url, "alice", "secret", &http).unwrap_err();
        assert!(matches!(error, NetError::TooLarge(_)));
        let _ = server.join().unwrap();
    }
}
