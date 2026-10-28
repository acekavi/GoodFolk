//! A message as RFC 5322 raw MIME, for SES's `Content.Raw`: `multipart/mixed`, holding a `multipart/alternative` of
//! the text and the HTML, then each attachment. Every body is base64 in lines of 76 characters, every line ends in
//! CRLF, a subject or sender's name that is not ASCII is RFC 2047 encoded words, and an ASCII sender's name is a quoted
//! string. The message carries its `Date`
//! and a `Message-ID` in the sender's domain.

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use time::OffsetDateTime;
use time::format_description::well_known::Rfc2822;

use crate::dev::safe_filename;
use crate::{MailError, Message};

/// The most bytes of the subject one encoded word carries: its base64 (60 characters) and `=?UTF-8?B??=` stay within
/// RFC 2047's 75.
const WORD_BYTES: usize = 45;
/// RFC 2045's longest base64 line.
const LINE_CHARS: usize = 76;

/// `message` from `from` as raw MIME sent at `now`, with `id` (letters and digits, unique per message) making its
/// `Message-ID` and its parts' boundaries (which cannot occur in the message, as base64 and the headers here cannot
/// hold `=_`). `Permanent` when `from`, the recipient or the subject holds a line break, which would start a header of
/// its own.
pub(crate) fn raw(from: &str, message: &Message, id: &str, now: OffsetDateTime) -> Result<String, MailError> {
    for (name, value) in [("sender", from), ("recipient", &message.to), ("subject", &message.subject)] {
        if value.contains(['\r', '\n']) {
            return Err(MailError::Permanent(format!("the {name} holds a line break")));
        }
    }
    let (mixed, alternative) = (format!("=_mixed_{id}"), format!("=_alt_{id}"));
    let date = now.format(&Rfc2822).map_err(|error| MailError::Permanent(format!("dating the message: {error}")))?;
    let (sender, address) = match from.trim().rsplit_once('<') {
        Some((name, address)) => (unquote(name.trim()), address.trim().trim_end_matches('>').trim()),
        None => (String::new(), from.trim()),
    };
    let domain = address.rsplit_once('@').map_or("localhost", |(_, domain)| domain);
    let from = match sender.as_str() {
        "" => address.to_owned(),
        name if name.is_ascii() => format!("{} <{address}>", quoted(name)),
        name => format!("{} <{address}>", words(name)),
    };
    let mut mime = format!(
        "From: {from}\r\nTo: {}\r\nSubject: {}\r\nDate: {date}\r\nMessage-ID: <{id}@{domain}>\r\n\
         MIME-Version: 1.0\r\nContent-Type: multipart/mixed; boundary=\"{mixed}\"\r\n\r\n",
        message.to,
        words(&message.subject)
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

/// `text` (a subject or a display name) as it is when it is ASCII, else as encoded words, each of whole characters,
/// folded onto lines of their own.
fn words(text: &str) -> String {
    if text.is_ascii() {
        return text.to_owned();
    }
    let mut words = Vec::new();
    let mut word = String::new();
    for c in text.chars() {
        if word.len() + c.len_utf8() > WORD_BYTES {
            words.push(std::mem::take(&mut word));
        }
        word.push(c);
    }
    words.push(word);
    words.iter().map(|word| format!("=?UTF-8?B?{}?=", STANDARD.encode(word))).collect::<Vec<_>>().join("\r\n ")
}

/// A display name as written in `MAIL_FROM`: a quoted string loses its quotes and escapes.
fn unquote(name: &str) -> String {
    let Some(inner) = name.strip_prefix('"').and_then(|name| name.strip_suffix('"')) else { return name.to_owned() };
    let mut unquoted = String::with_capacity(inner.len());
    let mut chars = inner.chars();
    while let Some(c) = chars.next() {
        unquoted.push(if c == '\\' { chars.next().unwrap_or(c) } else { c });
    }
    unquoted
}

/// An ASCII display name as an RFC 5322 quoted string, so specials such as `,` and `@` stay part of the name.
fn quoted(name: &str) -> String {
    format!("\"{}\"", name.replace('\\', "\\\\").replace('"', "\\\""))
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
