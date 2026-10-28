use super::*;
use std::fs;

fn entry(alias: &str) -> HostEntry {
    HostEntry {
        alias: alias.into(),
        hostname: "203.0.113.5".into(),
        user: Some("dev".into()),
        port: Some(2222),
        identity: Some("~/.ssh/id ed".into()),
    }
}

#[test]
fn block_and_validation() {
    assert_eq!(
        render_host_block(&entry("build"), "\n"),
        "Host build\n    HostName 203.0.113.5\n    User dev\n    Port 2222\n    IdentityFile \"~/.ssh/id ed\"\n"
    );
    for bad in ["a b", "web*", "x\ny", "!bad", "q?", ""] {
        assert!(validate_alias(bad).is_err(), "{bad:?}");
    }
    let mut e = entry("ok");
    e.hostname = "h\nProxyCommand evil".into();
    assert!(e.validate().is_err());
    assert!(parse_port("0").is_err() && parse_port("70000").is_err());
    assert_eq!(parse_port(" 22 ").unwrap(), Some(22));
}

#[test]
fn append_keeps_existing_bytes_and_rejects_duplicate() {
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join(OWN_FILE);
    append_host(&own, &entry("one"), &[]).unwrap();
    let first = fs::read(&own).unwrap();
    append_host(&own, &entry("two"), &[]).unwrap();
    let second = fs::read(&own).unwrap();
    assert!(second.starts_with(&first));
    assert!(String::from_utf8_lossy(&second).contains("Host two"));

    // a duplicate in our own file and in the list of known ssh hosts: nothing is written
    assert!(matches!(append_host(&own, &entry("ONE"), &[]), Err(Error::Duplicate(_))));
    assert!(matches!(append_host(&own, &entry("user-host"), &["user-host".into()]), Err(Error::Duplicate(_))));
    assert_eq!(fs::read(&own).unwrap(), second);
}

#[cfg(unix)]
#[test]
fn own_file_is_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join(OWN_FILE);
    append_host(&own, &entry("one"), &[]).unwrap();
    assert_eq!(fs::metadata(&own).unwrap().permissions().mode() & 0o777, 0o600);
}

#[test]
fn include_first_line_keeps_bom_crlf_and_is_idempotent() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config");
    let user = b"\xEF\xBB\xBFHost work\r\n    HostName prodhost\r\nInclude config.d/*\r\n".to_vec();
    fs::write(&cfg, &user).unwrap();

    let backup = ensure_include(&cfg, INCLUDE_LINE).unwrap().unwrap().unwrap();
    assert_eq!(fs::read(&backup).unwrap(), user);
    let after = fs::read(&cfg).unwrap();
    let mut expected = b"\xEF\xBB\xBF".to_vec();
    expected.extend_from_slice(b"Include saysdfterm.conf\r\n");
    expected.extend_from_slice(&user[3..]);
    assert_eq!(after, expected);

    assert!(ensure_include(&cfg, INCLUDE_LINE).unwrap().is_none());
    assert_eq!(fs::read(&cfg).unwrap(), after);
}

#[test]
fn include_detects_existing_forms_and_lf() {
    assert!(has_include("Host a\n  Include ~/.ssh/saysdfterm.conf\n"));
    assert!(has_include("include=\"C:\\Users\\u\\.ssh\\saysdfterm.conf\"\n"));
    assert!(!has_include("# Include saysdfterm.conf\nInclude other.conf\n"));

    let dir = tempfile::tempdir().unwrap();
    let cfg = dir.path().join("config");
    fs::write(&cfg, "Host a\n").unwrap();
    ensure_include(&cfg, INCLUDE_LINE).unwrap();
    assert_eq!(fs::read_to_string(&cfg).unwrap(), "Include saysdfterm.conf\nHost a\n");
}

#[cfg(unix)]
#[test]
fn include_refuses_symlink() {
    let dir = tempfile::tempdir().unwrap();
    let real = dir.path().join("real");
    fs::write(&real, "Host a\n").unwrap();
    let link = dir.path().join("config");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    assert!(matches!(ensure_include(&link, INCLUDE_LINE), Err(Error::Symlink(_))));
    assert_eq!(fs::read_to_string(&real).unwrap(), "Host a\n");
}

/// 2.3: user config + config.d -> add -> only the Include line changes.
#[test]
fn add_flow_touches_only_include_line() {
    let dir = tempfile::tempdir().unwrap();
    let ssh = dir.path();
    fs::create_dir(ssh.join("config.d")).unwrap();
    let work = b"Host build\n  HostName prodhost\n".to_vec();
    fs::write(ssh.join("config.d").join("work"), &work).unwrap();
    let user = b"Host mine\n  HostName m\nInclude config.d/*\n".to_vec();
    fs::write(ssh.join("config"), &user).unwrap();

    append_host(&ssh.join(OWN_FILE), &entry("new"), &["mine".into(), "build".into()]).unwrap();
    assert!(!include_present(&ssh.join("config")));
    ensure_include(&ssh.join("config"), INCLUDE_LINE).unwrap();

    let after = fs::read(ssh.join("config")).unwrap();
    assert_eq!(&after[after.len() - user.len()..], &user[..]);
    assert_eq!(&after[..after.len() - user.len()], b"Include saysdfterm.conf\n");
    assert_eq!(fs::read(ssh.join("config.d").join("work")).unwrap(), work);
}

#[test]
fn meta_upsert_keeps_foreign_entries() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("saysdf").join(meta::FILE_NAME);
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(&p, r#"{"old": {"title": "Old", "x": 1}}"#).unwrap();
    let m = meta::Meta { title: "New".into(), group: "prod".into(), ..Default::default() };
    meta::upsert(&p, "new", &m).unwrap();
    let v = meta::read(&p).unwrap();
    assert_eq!(v["old"]["x"], 1);
    assert_eq!(v["new"]["title"], "New");
    assert!(v["new"].get("description").is_none());
    let bad = meta::Meta { description: "a\nb".into(), ..Default::default() };
    assert!(meta::upsert(&p, "z", &bad).is_err());
}

#[test]
fn hostname_and_user_validation() {
    for ok in ["example.com", "203.0.113.10", "::1", "fe80::1", "my_host-1"] {
        assert!(validate_hostname(ok).is_ok(), "{ok}");
    }
    for bad in ["deploy@203.0.113.10", "a b", "$env:X", "%H%", "\"h\"", "h'", ""] {
        assert!(validate_hostname(bad).is_err(), "{bad}");
    }
    assert!(validate_hostname("a@b").unwrap_err().to_string().contains("User"));
    for bad in ["$env:USERPROFILE", "a@b", "%u%", "a b", "\"x\""] {
        assert!(validate_user(bad).is_err(), "{bad}");
    }
    assert!(validate_user("deploy").is_ok());
    assert!(parse_port("65535").is_ok() && parse_port("0").is_err() && parse_port("65536").is_err());
}

#[test]
fn normalize_splits_user_and_expands_identity() {
    let get = |n: &str| match n.to_ascii_uppercase().as_str() {
        "USERPROFILE" => Some("C:\\Users\\me".to_string()),
        _ => None,
    };
    let mut e = entry("x");
    e.hostname = "deploy@203.0.113.10".into();
    e.user = None;
    e.identity = Some("$env:USERPROFILE\\.ssh\\prod_key".into());
    let notes = e.normalize_with(&get);
    assert_eq!(e.hostname, "203.0.113.10");
    assert_eq!(e.user.as_deref(), Some("deploy"));
    assert_eq!(e.identity.as_deref(), Some("C:/Users/me/.ssh/prod_key"));
    assert_eq!(notes.len(), 2);
    e.validate().unwrap();
    assert!(!e.warnings().is_empty()); // no such file: a warning, not an error
    assert_eq!(expand_path_with("%USERPROFILE%\\k", &get), "C:\\Users\\me\\k");
    assert_eq!(expand_path_with("~/.ssh/k", &get), "C:\\Users\\me/.ssh/k");
    assert_eq!(expand_path_with("%NOPE%/k", &get), "%NOPE%/k");
}

#[test]
fn parse_pasted_ssh_command() {
    let c = parse_ssh_command(r#"ssh -i "$env:USERPROFILE\.ssh\prod_key" deploy@203.0.113.10"#).unwrap();
    assert_eq!(c.user.as_deref(), Some("deploy"));
    assert_eq!(c.host, "203.0.113.10");
    assert_eq!(c.identity.as_deref(), Some(r"$env:USERPROFILE\.ssh\prod_key"));
    let c = parse_ssh_command("ssh -p 2200 -i 'a b/key' u@h.example").unwrap();
    assert_eq!((c.port, c.identity.as_deref()), (Some(2200), Some("a b/key")));
    assert!(parse_ssh_command("ssh -p 99999 u@h").is_err());
    assert!(parse_ssh_command("ssh -i").is_err());
    assert!(parse_ssh_command("ssh u@h rm -rf /").is_err());
    assert!(parse_ssh_command("ssh -i \"abc u@h").is_err());
}

#[test]
fn bad_input_never_reaches_file() {
    let dir = tempfile::tempdir().unwrap();
    let own = dir.path().join(OWN_FILE);
    let mut e = entry("bad");
    e.hostname = "deploy@203.0.113.10".into();
    assert!(append_host(&own, &e, &[]).is_err());
    e.hostname = "h".into();
    e.user = Some("$env:USERPROFILE".into());
    assert!(append_host(&own, &e, &[]).is_err());
    assert!(!own.exists());
}

#[test]
fn validation_text_follows_language() {
    {
        let _lang = saysdf_i18n::testing::use_lang("en");
        let e = validate_hostname("deploy@203.0.113.10").unwrap_err().to_string();
        assert!(e.starts_with("host: HostName must not contain \"@\""), "{e}");
        assert_eq!(Error::Duplicate("prodhost".into()).to_string(), "alias \"prodhost\" is already in ssh config");
    }
    let _lang = saysdf_i18n::testing::use_lang("ru");
    let e = validate_hostname("deploy@203.0.113.10").unwrap_err().to_string();
    assert!(e.starts_with("host: в HostName не должно быть «@»"), "{e}"); // i18n-allow-cyrillic
    assert_eq!(
        Error::Duplicate("prodhost".into()).to_string(),
        "алиас «prodhost» уже есть в ssh config" // i18n-allow-cyrillic
    );
    assert_eq!(parse_port("0").unwrap_err().to_string(), "port: порт — число от 1 до 65535"); // i18n-allow-cyrillic
}
