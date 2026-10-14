//! Small conversions a developer keeps looking up on the web: Base64, URL encoding,
//! JWT, JSON, Unix time, UUID, hashes. All done here, nothing leaves the computer.

use std::hash::{BuildHasher, Hasher};

const ALPHABET: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

pub fn base64_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let value = chunk.iter().enumerate().fold(0u32, |value, (index, byte)| {
            value | (u32::from(*byte) << (16 - 8 * index))
        });
        for index in 0..4 {
            if index <= chunk.len() {
                out.push(ALPHABET[(value >> (18 - 6 * index) & 63) as usize] as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Reads standard and URL-safe Base64, with or without padding, spaces ignored.
pub fn base64_decode(text: &str) -> Result<Vec<u8>, String> {
    let mut bits = 0u32;
    let mut count = 0;
    let mut out = Vec::new();
    for ch in text.chars().filter(|ch| !ch.is_whitespace() && *ch != '=') {
        let value = match ch {
            'A'..='Z' => ch as u32 - 'A' as u32,
            'a'..='z' => ch as u32 - 'a' as u32 + 26,
            '0'..='9' => ch as u32 - '0' as u32 + 52,
            '+' | '-' => 62,
            '/' | '_' => 63,
            _ => return Err(format!("'{ch}' is not Base64")),
        };
        bits = bits << 6 | value;
        count += 6;
        if count >= 8 {
            count -= 8;
            out.push((bits >> count) as u8);
            bits &= (1 << count) - 1;
        }
    }
    Ok(out)
}

/// Percent-encoding of everything but the unreserved characters of RFC 3986.
pub fn url_encode(text: &str) -> String {
    text.bytes()
        .map(|byte| match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                (byte as char).to_string()
            }
            _ => format!("%{byte:02X}"),
        })
        .collect()
}

pub fn url_decode(text: &str) -> Result<String, String> {
    let bytes = text.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        match bytes[index] {
            b'%' => {
                let hex = text
                    .get(index + 1..index + 3)
                    .ok_or("A % lacks its two digits")?;
                let byte =
                    u8::from_str_radix(hex, 16).map_err(|_| format!("%{hex} is not a byte"))?;
                out.push(byte);
                index += 3;
            }
            b'+' => {
                out.push(b' ');
                index += 1;
            }
            byte => {
                out.push(byte);
                index += 1;
            }
        }
    }
    String::from_utf8(out).map_err(|_| "The bytes are not UTF-8 text".to_owned())
}

pub fn json_pretty(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    serde_json::to_string_pretty(&value).map_err(|err| err.to_string())
}

pub fn json_minify(text: &str) -> Result<String, String> {
    let value: serde_json::Value = serde_json::from_str(text).map_err(|err| err.to_string())?;
    serde_json::to_string(&value).map_err(|err| err.to_string())
}

/// What a JSON Web Token says. The signature is not checked: no key is known here.
#[derive(Debug, PartialEq, Eq)]
pub struct Jwt {
    pub header: String,
    pub payload: String,
    /// `exp` and `iat`, read as dates.
    pub expires: Option<i64>,
    pub issued: Option<i64>,
}

pub fn jwt_decode(token: &str) -> Result<Jwt, String> {
    let token = token.trim().trim_start_matches("Bearer ").trim();
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() != 3 {
        return Err("A JWT has three parts separated by dots".into());
    }
    let part = |index: usize| -> Result<serde_json::Value, String> {
        let bytes = base64_decode(parts[index])?;
        serde_json::from_slice(&bytes)
            .map_err(|err| format!("Part {} is not JSON: {err}", index + 1))
    };
    let header = part(0)?;
    let payload = part(1)?;
    let date = |key: &str| payload.get(key).and_then(serde_json::Value::as_i64);
    let pretty =
        |value: &serde_json::Value| serde_json::to_string_pretty(value).unwrap_or_default();
    Ok(Jwt {
        header: pretty(&header),
        payload: pretty(&payload),
        expires: date("exp"),
        issued: date("iat"),
    })
}

/// Days since 1970-01-01 → (year, month, day). Howard Hinnant's civil-from-days.
fn civil(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let day_of_era = z.rem_euclid(146_097);
    let year_of_era =
        (day_of_era - day_of_era / 1460 + day_of_era / 36_524 - day_of_era / 146_096) / 365;
    let day_of_year = day_of_era - (365 * year_of_era + year_of_era / 4 - year_of_era / 100);
    let shifted = (5 * day_of_year + 2) / 153;
    let day = (day_of_year - (153 * shifted + 2) / 5 + 1) as u32;
    let month = if shifted < 10 {
        shifted + 3
    } else {
        shifted - 9
    } as u32;
    let year = year_of_era + era * 400 + i64::from(month <= 2);
    (year, month, day)
}

fn days_from_civil(year: i64, month: u32, day: u32) -> i64 {
    let year = year - i64::from(month <= 2);
    let era = year.div_euclid(400);
    let year_of_era = year.rem_euclid(400);
    let month = i64::from(month);
    let day_of_year =
        (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + i64::from(day) - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    era * 146_097 + day_of_era - 719_468
}

/// Seconds (or milliseconds, when the number is that big) → `2026-10-14 16:30:00 UTC`.
pub fn timestamp_to_date(stamp: i64) -> String {
    let seconds = if stamp.abs() > 100_000_000_000 {
        stamp / 1000
    } else {
        stamp
    };
    let (year, month, day) = civil(seconds.div_euclid(86_400));
    let rest = seconds.rem_euclid(86_400);
    format!(
        "{year:04}-{month:02}-{day:02} {:02}:{:02}:{:02} UTC",
        rest / 3600,
        rest / 60 % 60,
        rest % 60
    )
}

/// `2026-10-14`, `2026-10-14 16:30`, `2026-10-14T16:30:00Z` (UTC) → Unix seconds.
pub fn date_to_timestamp(text: &str) -> Result<i64, String> {
    let text = text.trim().trim_end_matches('Z').trim_end_matches(" UTC");
    let (date, time) = text.split_once(['T', ' ']).unwrap_or((text, "00:00:00"));
    let numbers = |part: &str| -> Vec<i64> {
        part.split([':', '-', '.'])
            .filter_map(|n| n.parse().ok())
            .collect()
    };
    let date = numbers(date);
    let time = numbers(time);
    let [year, month, day] = date[..] else {
        return Err("Write the date as YYYY-MM-DD".into());
    };
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return Err("No such month or day".into());
    }
    let at = |index: usize| time.get(index).copied().unwrap_or(0);
    Ok(
        days_from_civil(year, month as u32, day as u32) * 86_400
            + at(0) * 3600
            + at(1) * 60
            + at(2),
    )
}

/// Random UUID (version 4) from the random keys Rust gives each hasher. Fine for ids, not
/// for secrets.
pub fn uuid_v4() -> String {
    let random = || {
        let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write_u128(
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |time| time.as_nanos()),
        );
        hasher.finish()
    };
    let mut bytes = [0u8; 16];
    bytes[..8].copy_from_slice(&random().to_le_bytes());
    bytes[8..].copy_from_slice(&random().to_le_bytes());
    bytes[6] = bytes[6] & 0x0F | 0x40;
    bytes[8] = bytes[8] & 0x3F | 0x80;
    let hex: String = bytes.iter().map(|byte| format!("{byte:02x}")).collect();
    format!(
        "{}-{}-{}-{}-{}",
        &hex[..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..]
    )
}

/// MD5, SHA-1 and SHA-256 of the text, in hexadecimal.
pub fn hashes(text: &str) -> Vec<(&'static str, String)> {
    use sha2::Digest as _;
    let hex = |bytes: &[u8]| {
        bytes
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>()
    };
    vec![
        ("MD5", hex(&md5::Md5::digest(text.as_bytes()))),
        ("SHA-1", hex(&sha1::Sha1::digest(text.as_bytes()))),
        ("SHA-256", hex(&sha2::Sha256::digest(text.as_bytes()))),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn base64_both_ways() {
        assert_eq!(base64_encode(b"hello world"), "aGVsbG8gd29ybGQ=");
        assert_eq!(base64_encode(b"ab"), "YWI=");
        assert_eq!(base64_decode("aGVsbG8gd29ybGQ=").unwrap(), b"hello world");
        assert_eq!(base64_decode("YWI").unwrap(), b"ab");
        assert!(base64_decode("a$b").is_err());
    }

    #[test]
    fn url_both_ways() {
        assert_eq!(url_encode("a b&c=é"), "a%20b%26c%3D%C3%A9");
        assert_eq!(url_decode("a%20b+c%C3%A9").unwrap(), "a b cé");
        assert!(url_decode("%G1").is_err());
    }

    #[test]
    fn reads_a_jwt() {
        let token = "eyJhbGciOiJIUzI1NiIsInR5cCI6IkpXVCJ9.eyJzdWIiOiIxMjM0NTY3ODkwIiwibmFtZSI6IkpvaG4gRG9lIiwiaWF0IjoxNTE2MjM5MDIyfQ.SflKxwRJSMeKKF2QT4fwpMeJf36POk6yJV_adQssw5c";
        let jwt = jwt_decode(&format!("Bearer {token}")).unwrap();
        assert!(jwt.header.contains("\"HS256\""));
        assert!(jwt.payload.contains("\"John Doe\""));
        assert_eq!(jwt.issued, Some(1_516_239_022));
        assert!(jwt_decode("abc").is_err());
    }

    #[test]
    fn dates_both_ways() {
        assert_eq!(timestamp_to_date(0), "1970-01-01 00:00:00 UTC");
        assert_eq!(timestamp_to_date(1_516_239_022), "2018-01-18 01:30:22 UTC");
        assert_eq!(
            timestamp_to_date(1_516_239_022_000),
            "2018-01-18 01:30:22 UTC"
        );
        assert_eq!(
            date_to_timestamp("2018-01-18T01:30:22Z").unwrap(),
            1_516_239_022
        );
        assert_eq!(date_to_timestamp("2000-03-01").unwrap(), 951_868_800);
        assert!(date_to_timestamp("2000-13-01").is_err());
    }

    #[test]
    fn json_and_uuid_and_hashes() {
        assert_eq!(json_minify("{ \"a\" : [1, 2] }").unwrap(), "{\"a\":[1,2]}");
        assert!(json_pretty("{\"a\":1}").unwrap().contains("\n  \"a\": 1"));
        assert!(json_pretty("{").is_err());
        let uuid = uuid_v4();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        assert_ne!(uuid, uuid_v4());
        let hashes = hashes("abc");
        assert_eq!(hashes[0].1, "900150983cd24fb0d6963f7d28e17f72");
        assert_eq!(
            hashes[2].1,
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
    }
}
