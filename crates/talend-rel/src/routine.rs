//! Custom routines whose body the lowering recognises (EKOS RFC 0267 phase 2 §Custom routines).
//!
//! A routine is project Java code; the lowering does not interpret Java bodies. It recognises one
//! shape, found in every custom routine of the fixtures: a digest of the method's one `String`
//! parameter, written as two hex digits per byte. Anything else stays a gap naming the routine.

use ekos_lakehouse_etl_expr::Func;
use ekos_talend::Routine;

/// A recognised digest routine method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Digest {
    pub func: Func,
    /// `String.format("%02X")`: the hex is upper case.
    pub upper: bool,
    /// The body returns null for a null input; otherwise a null stops the job.
    pub null_safe: bool,
    /// `getBytes("UTF-8")` / `StandardCharsets.UTF_8`; otherwise the platform's charset.
    pub utf8: bool,
}

/// The method `class.method` of a custom routine, when its body is a digest.
pub fn digest(routines: &[Routine], class: &str, method: &str) -> Option<Digest> {
    let r = routines.iter().find(|r| !r.system && r.name == class)?;
    let source = r.source.as_deref()?;
    let (param, body) = method_body(source, method)?;
    let compact: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let func = ["MD5", "SHA-1", "SHA1", "SHA-256", "SHA256"]
        .into_iter()
        .find(|a| compact.contains(&format!("MessageDigest.getInstance(\"{a}\")")))
        .map(|a| match a {
            "MD5" => Func::Md5Hex,
            "SHA-1" | "SHA1" => Func::Sha1Hex,
            _ => Func::Sha256Hex,
        })?;
    if compact.matches("MessageDigest.getInstance(").count() != 1
        || !compact.contains(&format!(".digest({param}.getBytes("))
    {
        return None;
    }
    // Two hex digits per byte, zero-padded.
    let upper = if compact.contains("String.format(\"%02X\"") {
        true
    } else if compact.contains("String.format(\"%02x\"")
        || (compact.contains("Integer.toHexString(")
            && (compact.contains("|0x100).substring(1,3)") || compact.contains(".length()==1)")))
    {
        false
    } else {
        return None;
    };
    let null_safe = compact.contains(&format!("if({param}==null)returnnull"))
        || compact.contains(&format!("if({param}==null){{returnnull"));
    let utf8 = compact.contains("getBytes(\"UTF-8\")")
        || compact.contains("getBytes(StandardCharsets.UTF_8)")
        || compact.contains("getBytes(java.nio.charset.StandardCharsets.UTF_8)");
    Some(Digest {
        func,
        upper,
        null_safe,
        utf8,
    })
}

/// `public static String <method>(String <p>) { … }`: the parameter's name and the body.
fn method_body<'a>(source: &'a str, method: &str) -> Option<(String, &'a str)> {
    let mut from = 0;
    while let Some(at) = source[from..].find(method) {
        let start = from + at;
        from = start + method.len();
        let before = source[..start].trim_end();
        if !before.ends_with("String") || !source[..start].contains("static") {
            continue;
        }
        let rest = &source[start + method.len()..];
        let rest_trim = rest.trim_start();
        let Some(params) = rest_trim.strip_prefix('(') else {
            continue;
        };
        let close = params.find(')')?;
        let p: Vec<&str> = params[..close].split_whitespace().collect();
        let [ty, name] = p.as_slice() else {
            return None;
        };
        if *ty != "String" && *ty != "java.lang.String" {
            return None;
        }
        let after = &params[close + 1..];
        let open = after.find('{')?;
        let body = &after[open..];
        let mut depth = 0usize;
        for (i, ch) in body.char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(((*name).to_string(), &body[1..i]));
                    }
                }
                _ => {}
            }
        }
        return None;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn routine(source: &str) -> Vec<Routine> {
        vec![Routine {
            name: "HashUtil".into(),
            version: "0.1".into(),
            path: "code/routines/HashUtil_0.1.item".into(),
            system: false,
            source: Some(source.into()),
            methods: vec!["h".into()],
        }]
    }

    #[test]
    fn digest_bodies_are_recognised_with_their_case_null_and_charset() {
        let upper = routine(
            r#"public class HashUtil { public static String h(String input) { try {
            MessageDigest md = MessageDigest.getInstance("MD5");
            byte[] d = md.digest(input.getBytes());
            StringBuilder sb = new StringBuilder();
            for (byte b : d) { sb.append(String.format("%02X", b)); }
            return sb.toString(); } catch (Exception e) { throw new RuntimeException(e); } } }"#,
        );
        let d = digest(&upper, "HashUtil", "h").unwrap();
        assert_eq!(d.func, Func::Md5Hex);
        assert!(d.upper && !d.null_safe && !d.utf8);
        let padded = routine(
            r#"public class HashUtil { public static String h(String input) {
            if (input == null) { return null; }
            MessageDigest md = MessageDigest.getInstance("SHA-256");
            byte[] hash = md.digest(input.getBytes("UTF-8"));
            StringBuilder hexString = new StringBuilder();
            for (byte b : hash) { String hex = Integer.toHexString(0xff & b);
              if(hex.length() == 1) hexString.append('0'); hexString.append(hex); }
            return hexString.toString(); } }"#,
        );
        let d = digest(&padded, "HashUtil", "h").unwrap();
        assert_eq!(d.func, Func::Sha256Hex);
        assert!(!d.upper && d.null_safe && d.utf8);
    }

    #[test]
    fn an_unpadded_hex_or_another_body_is_not_a_digest() {
        let unpadded = routine(
            r#"public class HashUtil { public static String h(String input) {
            MessageDigest md = MessageDigest.getInstance("MD5");
            byte[] d = md.digest(input.getBytes());
            StringBuilder sb = new StringBuilder();
            for (byte b : d) { sb.append(Integer.toHexString(0xff & b)); }
            return sb.toString(); } }"#,
        );
        assert_eq!(digest(&unpadded, "HashUtil", "h"), None);
        let other = routine(
            "public class HashUtil { public static String h(String input) { return input.trim(); } }",
        );
        assert_eq!(digest(&other, "HashUtil", "h"), None);
        assert_eq!(digest(&other, "Other", "h"), None);
    }
}
