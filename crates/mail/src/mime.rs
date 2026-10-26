//! A message as RFC 5322 raw MIME, for SES's `Content.Raw`: `multipart/mixed`, holding a `multipart/alternative` of
//! the text and the HTML, then each attachment. Every body is base64 in lines of 76 characters, every line ends in
//! CRLF, and a subject that is not ASCII is RFC 2047 encoded words.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;

use crate::dev::safe_filename;
use crate::{MailError, Message};

/// The most bytes of the subject one encoded word carries: its base64 (60 characters) and `=?UTF-8?B??=` stay within
/// RFC 2047's 75.
const WORD_BYTES: usize = 45;
/// RFC 2045's longest base64 line.
const LINE_CHARS: usize = 76;

/// `message` from `from` as raw MIME, its parts separated by boundaries made from `boundary` (which must not occur in
/// the message, as base64 and the headers here cannot hold `=_`). `Permanent` when `from`, the recipient or the
/// subject holds a line break, which would start a header of its own.
pub(crate) fn raw(from: &str, message: &Message, boundary: &str) -> Result<String, MailError> {
    for (name, value) in [("sender", from), ("recipient", &message.to), ("subject", &message.subject)] {
        if value.contains(['\r', '\n']) {
            return Err(MailError::Permanent(format!("the {name} holds a line break")));
        }
    }
    let (mixed, alternative) = (format!("=_mixed_{boundary}"), format!("=_alt_{boundary}"));
    let mut mime = format!(
        "From: {from}\r\nTo: {}\r\nSubject: {}\r\nMIME-Version: 1.0\r\n\
         Content-Type: multipart/mixed; boundary=\"{mixed}\"\r\n\r\n",
        message.to,
        subject(&message.subject)
    );
    mime.push_str(&format!("--{mixed}\r\nContent-Type: multipart/alternative; boundary=\"{alternative}\"\r\n\r\n"));
    for (content_type, body) in [("text/plain", &message.text), ("text/html", &message.html)] {
        mime.push_str(&format!(
            "--{alternative}\r\nContent-Type: {content_type}; charset=UTF-8\r\nContent-Transfer-Encoding: base64\r\n\r\n"
        ));
        mime.push_str(&base64_lines(body.as_bytes()));
    }
    mime.push_str(&format!("--{alternative}--\r\n"));
    for attachment in &message.attachments {
        let filename = safe_filename(&attachment.filename);
        mime.push_str(&format!(
            "--{mixed}\r\nContent-Type: {}; name=\"{filename}\"\r\n\
             Content-Disposition: attachment; filename=\"{filename}\"\r\nContent-Transfer-Encoding: base64\r\n\r\n",
            media_type(&attachment.content_type)
        ));
        mime.push_str(&base64_lines(&attachment.bytes));
    }
    mime.push_str(&format!("--{mixed}--\r\n"));
    Ok(mime)
}

/// The subject as it is when it is ASCII, else as encoded words, each of whole characters, folded onto lines of
/// their own.
fn subject(subject: &str) -> String {
    if subject.is_ascii() {
        return subject.to_owned();
    }
    let mut words = Vec::new();
    let mut word = String::new();
    for c in subject.chars() {
        if word.len() + c.len_utf8() > WORD_BYTES {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    words.push(word);
    words.iter().map(|word| format!("=?UTF-8?B?{}?=", STANDARD.encode(word))).collect::<Vec<_>>().join("\r\n ")
}

/// `bytes` in base64, in lines of at most 76 characters, each ending in CRLF.
fn base64_lines(bytes: &[u8]) -> String {
    let encoded = STANDARD.encode(bytes);
    let mut lines = String::with_capacity(encoded.len() + encoded.len() / LINE_CHARS * 2 + 2);
    for line in encoded.as_bytes().chunks(LINE_CHARS) {
        lines.push_str(std::str::from_utf8(line).expect("base64 is ASCII"));
        lines.push_str("\r\n");
    }
    lines
}

/// `content_type` when it is a plain `type/subtype`, else `application/octet-stream`.
fn media_type(content_type: &str) -> &str {
    let token =
        |part: &str| !part.is_empty() && part.bytes().all(|b| b.is_ascii_alphanumeric() || b"!#$&^_.+-".contains(&b));
    match content_type.split_once('/') {
        Some((kind, subtype)) if token(kind) && token(subtype) => content_type,
        _ => "application/octet-stream",
    }
}
