//! "Connect everything": the links between the apps that a fresh install
//! lacks. qBittorrent's Web UI (Sonarr and Radarr talk to it through that),
//! qBittorrent as the download client in Sonarr and Radarr, Sonarr and Radarr
//! as apps in Prowlarr, and FlareSolverr as Prowlarr's proxy. Only what is
//! missing is added; anything the user already set is left as it is.

use serde_json::Value;

/// Sets one `key=value` in a section of an ini file, adding either if missing,
/// and keeps everything else as it was.
pub fn set_ini_value(ini: &str, section: &str, key: &str, value: &str) -> String {
    let mut out: Vec<String> = Vec::new();
    let (mut cur, mut seen_section, mut done) = (String::new(), false, false);
    let prefix = format!("{key}=");
    for line in ini.lines() {
        let t = line.trim();
        if t.starts_with('[') && t.ends_with(']') {
            if cur == section && !done {
                out.push(format!("{prefix}{value}"));
                done = true;
            }
            cur = t[1..t.len() - 1].to_string();
            seen_section |= cur == section;
        } else if cur == section && t.starts_with(&prefix) {
            if !done {
                out.push(format!("{prefix}{value}"));
                done = true;
            }
            continue;
        }
        out.push(line.to_string());
    }
    if !done {
        if !seen_section {
            if out.last().is_some_and(|l| !l.trim().is_empty()) {
                out.push(String::new());
            }
            out.push(format!("[{section}]"));
        }
        out.push(format!("{prefix}{value}"));
    }
    out.join("\n") + "\n"
}

/// qBittorrent.ini with the Web UI switched on, no password for programs on
/// this PC, and the first-run legal notice accepted. qBittorrent 5 reads the
/// `[WebUI]` section; older builds read `WebUI\...` under `[Preferences]`.
/// Both are written (each ignores the other), with the same port: one already
/// chosen is kept. qBittorrent 5 won't start the Web UI at all without a login
/// ("WebUI: credentials are not set" in its log), even with LocalHostAuth off,
/// so one is added when missing. Its password is random and never kept: Chillarr,
/// Sonarr and Radarr come from this PC and skip it. Like the switch, the login
/// is written in both layouts: up to 5.2 it lives under `[Preferences]`, from 5.3
/// under `[WebUI]` (5.3 moves the old keys over only once, when it upgrades a
/// profile, so a fresh 5.3 reads `[WebUI]` alone). Unchanged when the switch is
/// on and a login exists, so a port or password the user picked is never touched.
pub fn enable_webui(ini: &str, port: u16) -> String {
    if webui_enabled_new(ini) && has_login(ini) {
        return ini.to_string();
    }
    let mut out = ini.to_string();
    if !has_login(ini) {
        let mut pass = [0u8; 24];
        if getrandom::getrandom(&mut pass).is_ok() {
            let pass: String = pass.iter().map(|b| format!("{b:02x}")).collect();
            if let Some(with_login) = set_qbit_login(&out, "chillarr", &pass) {
                out = with_login;
            }
        }
    }
    if webui_enabled_new(ini) {
        return out;
    }
    let port = crate::discover::parse_qbit_port(ini).unwrap_or(port).to_string();
    for (section, key, value) in [
        ("WebUI", "Enabled", "true"),
        ("WebUI", "LocalHostAuth", "false"),
        ("WebUI", "Port", port.as_str()),
        ("Preferences", "WebUI\\Enabled", "true"),
        ("Preferences", "WebUI\\LocalHostAuth", "false"),
        ("Preferences", "WebUI\\Port", port.as_str()),
        ("LegalNotice", "Accepted", "true"),
    ] {
        out = set_ini_value(&out, section, key, value);
    }
    out
}

/// The `[WebUI] Enabled=true` that qBittorrent 5 reads.
fn webui_enabled_new(ini: &str) -> bool {
    let mut section = "";
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
        } else if section == "WebUI" && line.eq_ignore_ascii_case("Enabled=true") {
            return true;
        }
    }
    false
}

/// A Web UI password is set, in either layout (`[WebUI]` from qBittorrent 5.3,
/// `[Preferences]` before it).
fn has_login(ini: &str) -> bool {
    let mut section = "";
    for line in ini.lines().map(str::trim) {
        if line.starts_with('[') && line.ends_with(']') {
            section = &line[1..line.len() - 1];
        } else if (section == "WebUI" && line.starts_with("Password_PBKDF2="))
            || (section == "Preferences" && line.starts_with("WebUI\\Password_PBKDF2="))
        {
            return true;
        }
    }
    false
}

/// Why qBittorrent's Web UI didn't start, from its own log: the last critical
/// `(C)` line about the WebUI stamped at or after `since` (the log's local time,
/// `YYYY-MM-DDTHH:MM:SS`, which sorts as text). qBittorrent writes it in its own
/// language, so it is passed on as it stands.
pub fn qbit_webui_error(log: &str, since: &str) -> Option<String> {
    log.lines().rev().find_map(|line| {
        let rest = line.trim().strip_prefix("(C) ")?;
        let (stamp, msg) = rest.split_once(" - ")?;
        (stamp >= since && msg.contains("WebUI")).then(|| msg.trim().to_string())
    })
}

/// qBittorrent.ini with this Web UI login, in both layouts (see `enable_webui`),
/// replacing any login there. None only if no random salt could be had.
pub fn set_qbit_login(ini: &str, user: &str, password: &str) -> Option<String> {
    let mut salt = [0u8; 16];
    getrandom::getrandom(&mut salt).ok()?;
    let hash = password_value(password, &salt);
    let mut out = set_ini_value(ini, "WebUI", "Username", user);
    out = set_ini_value(&out, "WebUI", "Password_PBKDF2", &hash);
    out = set_ini_value(&out, "Preferences", "WebUI\\Username", user);
    out = set_ini_value(&out, "Preferences", "WebUI\\Password_PBKDF2", &hash);
    Some(out)
}

/// A password as qBittorrent stores it: PBKDF2-HMAC-SHA512, 100,000 rounds,
/// 16-byte salt, 64-byte key, written as `"@ByteArray(<salt>:<key>)"` in base64.
pub fn password_value(password: &str, salt: &[u8; 16]) -> String {
    use base64::Engine;
    let mut key = [0u8; 64];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(password.as_bytes(), salt, 100_000, &mut key);
    let b64 = base64::engine::general_purpose::STANDARD;
    format!("\"@ByteArray({}:{})\"", b64.encode(salt), b64.encode(key))
}

/// Sets each named field's value in an `*arr` schema entry.
pub fn fill_fields(mut item: Value, values: &[(&str, Value)]) -> Value {
    if let Some(fields) = item["fields"].as_array_mut() {
        for f in fields.iter_mut() {
            if let Some((_, v)) = values.iter().find(|(n, _)| f["name"] == *n) {
                f["value"] = v.clone();
            }
        }
    }
    item
}

pub fn has_impl(list: &[Value], implementation: &str) -> bool {
    list.iter().any(|x| x["implementation"] == implementation)
}

pub fn pick_schema(schemas: &[Value], implementation: &str) -> Option<Value> {
    schemas.iter().find(|x| x["implementation"] == implementation).cloned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ini_values_are_set_in_place_or_added() {
        let ini = "[BitTorrent]\nSession\\Port=1\nSession\\DefaultSavePath=C:/old\n\n[Preferences]\nA=1\n";
        let out = set_ini_value(ini, "BitTorrent", "Session\\DefaultSavePath", "D:/Media/Downloads");
        assert!(out.contains("Session\\DefaultSavePath=D:/Media/Downloads") && !out.contains("C:/old") && out.contains("Session\\Port=1") && out.contains("A=1"));
        let added = set_ini_value("[Preferences]\nA=1\n", "BitTorrent", "Session\\DefaultSavePath", "D:/x");
        assert!(added.contains("[BitTorrent]\nSession\\DefaultSavePath=D:/x"));
        let new_key = set_ini_value("[BitTorrent]\nSession\\Port=1\n[Other]\nB=2\n", "BitTorrent", "Session\\DefaultSavePath", "D:/y");
        assert!(new_key.find("Session\\DefaultSavePath=D:/y").unwrap() < new_key.find("[Other]").unwrap());
    }
    use serde_json::json;

    #[test]
    fn webui_turned_on_for_a_fresh_qbittorrent() {
        let out = enable_webui("", 8080);
        assert!(out.contains("[Preferences]"));
        assert!(out.contains("WebUI\\Enabled=true"));
        assert!(out.contains("WebUI\\Port=8080"));
        assert!(out.contains("WebUI\\LocalHostAuth=false"), "Sonarr/Radarr on this PC connect without a password");
        assert!(out.contains("[LegalNotice]\nAccepted=true"), "first run must not stop at the legal notice");
    }

    #[test]
    fn webui_without_a_login_gets_one() {
        // What Chillarr 0.3.2 left behind: switched on, but qBittorrent 5 refuses to start it.
        let ini = "[WebUI]\nEnabled=true\nPort=8080\nLocalHostAuth=false\n\n[Preferences]\nWebUI\\Enabled=true\nWebUI\\Port=8080\n";
        let out = enable_webui(ini, 8080);
        assert!(out.contains("WebUI\\Username=chillarr"), "{out}");
        assert!(out.contains("WebUI\\Password_PBKDF2=\"@ByteArray("), "{out}");
        assert_eq!(out.matches("Port=8080").count(), 2, "nothing else changes");
        // Two runs never share a salt or password.
        assert_ne!(enable_webui(ini, 8080), out);
        // A login the user set is kept.
        assert_eq!(enable_webui(&out, 8080), out);
    }

    #[test]
    fn login_lands_in_both_layouts() {
        // A fresh qBittorrent 5.3 reads the login only from [WebUI].
        let out = enable_webui("", 8080);
        let webui = out.split("[WebUI]").nth(1).unwrap().split("\n[").next().unwrap();
        assert!(webui.contains("\nUsername=chillarr") && webui.contains("\nPassword_PBKDF2="), "{out}");
        assert!(out.contains("WebUI\\Username=chillarr") && out.contains("WebUI\\Password_PBKDF2="), "{out}");
        // Both copies are the same password.
        let hashes: Vec<&str> = out.lines().filter_map(|l| l.split_once("Password_PBKDF2=").map(|x| x.1)).collect();
        assert_eq!(hashes.len(), 2);
        assert_eq!(hashes[0], hashes[1]);
    }

    #[test]
    fn a_chosen_login_replaces_the_old_one_in_both_layouts() {
        let ini = "[WebUI]\nPort=6767\nEnabled=true\nUsername=chillarr\nPassword_PBKDF2=\"@ByteArray(x:y)\"\n";
        let out = set_qbit_login(ini, "sam", "hunter22").unwrap();
        assert!(!out.contains("chillarr") && !out.contains("@ByteArray(x:y)"), "{out}");
        assert_eq!(out.matches("Username=sam").count(), 2, "{out}");
        assert!(out.contains("Port=6767\nEnabled=true"), "nothing else changes:\n{out}");
        // It's still "a login exists", so Fix everything leaves it alone.
        assert_eq!(enable_webui(&out, 8080), out);
    }

    #[test]
    fn a_53_login_counts_as_a_login() {
        // The user's own 5.3 setup: login under [WebUI] only. Never touched.
        let ini = "[WebUI]\nPort=6767\nEnabled=true\nPassword_PBKDF2=\"@ByteArray(x:y)\"\nLocalHostAuth=false\n";
        assert_eq!(enable_webui(ini, 8080), ini);
    }

    #[test]
    fn webui_error_comes_from_the_log() {
        let log = "(N) 2026-10-17T16:41:05 - Using built-in WebUI.\n\
                   (C) 2026-10-17T10:00:00 - WebUI: old failure\n\
                   (C) 2026-10-17T16:41:06 - WebUI: As credenciais nao estao definidas\n\
                   (C) 2026-10-17T16:41:07 - Something else broke\n";
        assert_eq!(
            qbit_webui_error(log, "2026-10-17T16:41:00").as_deref(),
            Some("WebUI: As credenciais nao estao definidas")
        );
        // Older than this start: not this start's problem.
        assert_eq!(qbit_webui_error(log, "2026-10-17T17:00:00"), None);
    }

    #[test]
    fn password_is_stored_the_way_qbittorrent_checks_it() {
        // Same as .NET Rfc2898DeriveBytes("adminadmin", 0..15, 100000, SHA512).
        let salt: [u8; 16] = core::array::from_fn(|i| i as u8);
        assert_eq!(
            password_value("adminadmin", &salt),
            "\"@ByteArray(AAECAwQFBgcICQoLDA0ODw==:UC4grCR8uxnR4Gzyoule02IP9X8mRb7SaEfCylvs19kyzc76Vdb/0xQlSKu43sz2Dl4vz2SIZymUO3PvTxIfVQ==)\""
        );
    }

    #[test]
    fn webui_already_on_is_left_alone() {
        // The user's own setup: never change a port or password they chose.
        let ini = "[LegalNotice]\nAccepted=true\n\n[WebUI]\nPort=6767\nEnabled=true\n\n[Preferences]\nWebUI\\Password_PBKDF2=\"@ByteArray(x:y)\"\n";
        assert_eq!(enable_webui(ini, 8080), ini);
        // Only the old layout: qBittorrent 5 ignores it, so the [WebUI] switch is
        // added with the port already chosen there; nothing old is changed.
        let old = "[Preferences]\nWebUI\\Enabled=true\nWebUI\\Port=9090\n[LegalNotice]\nAccepted=true\n";
        let out = enable_webui(old, 8080);
        let webui = out.split("[WebUI]").nth(1).unwrap().split("\n[").next().unwrap();
        for line in ["\nEnabled=true", "\nLocalHostAuth=false", "\nPort=9090"] {
            assert!(webui.contains(line), "{line} missing:\n{out}");
        }
        assert!(out.contains("WebUI\\Port=9090") && !out.contains("Port=8080"));
    }

    #[test]
    fn webui_off_gets_switched_on_keeping_other_settings() {
        let ini = "[BitTorrent]\nSession\\DefaultSavePath=D:\\\\T\n\n[Preferences]\nGeneral\\Locale=en\n";
        let out = enable_webui(ini, 8080);
        assert!(out.contains("Session\\DefaultSavePath=D:\\\\T"));
        assert!(out.contains("General\\Locale=en"));
        assert!(out.contains("WebUI\\Enabled=true"));
        assert_eq!(out.matches("[Preferences]").count(), 1);
    }

    #[test]
    fn fills_schema_fields_by_name() {
        let schema = json!({"implementation":"QBittorrent","fields":[
            {"name":"host","value":"localhost"},{"name":"port","value":8080},{"name":"movieCategory","value":"radarr"}]});
        let out = fill_fields(schema, &[("port", json!(6767)), ("movieCategory", json!("films"))]);
        let f = out["fields"].as_array().unwrap();
        assert_eq!(f[1]["value"], 6767);
        assert_eq!(f[2]["value"], "films");
        assert_eq!(f[0]["value"], "localhost");
    }

    #[test]
    fn finds_an_existing_link_by_implementation() {
        let list = vec![json!({"implementation":"QBittorrent"}), json!({"implementation":"Sonarr"})];
        assert!(has_impl(&list, "Sonarr"));
        assert!(!has_impl(&list, "Radarr"));
    }

    #[test]
    fn picks_the_schema_entry() {
        let schemas = vec![json!({"implementation":"Transmission"}), json!({"implementation":"QBittorrent","name":""})];
        assert_eq!(pick_schema(&schemas, "QBittorrent").unwrap()["implementation"], "QBittorrent");
        assert!(pick_schema(&schemas, "Deluge").is_none());
    }
}
