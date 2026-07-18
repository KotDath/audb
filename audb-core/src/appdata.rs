use crate::app;
use crate::error::{CoreError, CoreResult};
use crate::transport::{shell_quote, EmulatorTransport};
use rusqlite::{types::ValueRef, Connection, OpenFlags};
use serde_json::{json, Value};
use std::path::{Component, Path};
use std::time::Duration;

fn component(value: &str, label: &str) -> CoreResult<()> {
    if value.is_empty()
        || !value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "_.-".contains(c))
    {
        Err(CoreError::invalid(format!("Invalid {label}: {value:?}")))
    } else {
        Ok(())
    }
}
pub async fn metadata(t: &mut EmulatorTransport, package: &str) -> CoreResult<Value> {
    app::validate_package(package)?;
    let paths = [
        format!("/usr/share/applications/{package}.desktop"),
        format!(
            "/home/{}/.local/share/applications/{package}.desktop",
            t.config().ssh_user
        ),
    ];
    let command=format!("for f in {} {}; do test -f \"$f\" || continue; echo DESKTOP=$f; sed -n 's/^OrganizationName=/ORGANIZATION=/p; s/^ApplicationName=/APPLICATION=/p' \"$f\"; break; done",shell_quote(&paths[0]),shell_quote(&paths[1]));
    let raw = t.exec(&command, true).await?;
    let mut desktop = None;
    let mut org = None;
    let mut application = None;
    for line in raw.lines() {
        if let Some(v) = line.strip_prefix("DESKTOP=") {
            desktop = Some(v.to_string())
        }
        if let Some(v) = line.strip_prefix("ORGANIZATION=") {
            org = Some(v.trim().to_string())
        }
        if let Some(v) = line.strip_prefix("APPLICATION=") {
            application = Some(v.trim().to_string())
        }
    }
    let org = org.ok_or_else(|| {
        CoreError::invalid(format!(
            "Cannot determine private data paths for {package}: desktop metadata not found"
        ))
    })?;
    let application = application.ok_or_else(|| {
        CoreError::invalid(format!(
            "Cannot determine private data paths for {package}: desktop metadata not found"
        ))
    })?;
    component(&org, "OrganizationName")?;
    component(&application, "ApplicationName")?;
    Ok(
        json!({"package":package,"desktopFile":desktop,"organization":org,"application":application}),
    )
}
pub async fn paths(t: &mut EmulatorTransport, package: &str) -> CoreResult<Value> {
    let mut m = metadata(t, package).await?;
    let org = m["organization"].as_str().unwrap();
    let app = m["application"].as_str().unwrap();
    let home = format!("/home/{}", t.config().ssh_user);
    let roots = [
        ("config", format!("{home}/.config/{org}/{app}")),
        ("cache", format!("{home}/.cache/{org}/{app}")),
        ("data", format!("{home}/.local/share/{org}/{app}")),
    ];
    let checks = roots
        .iter()
        .map(|(k, p)| {
            format!(
                "if test -e {}; then echo {k}=1; else echo {k}=0; fi",
                shell_quote(p)
            )
        })
        .collect::<Vec<_>>()
        .join(" ; ");
    let found = t.exec(&checks, true).await?;
    let list=roots.into_iter().map(|(kind,path)|json!({"kind":kind,"path":path,"exists":found.lines().any(|l|l==format!("{kind}=1"))})).collect();
    m["paths"] = Value::Array(list);
    Ok(m)
}
async fn roots(
    t: &mut EmulatorTransport,
    package: &str,
) -> CoreResult<std::collections::HashMap<String, String>> {
    Ok(paths(t, package).await?["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|v| Some((v["kind"].as_str()?.into(), v["path"].as_str()?.into())))
        .collect())
}
fn relative(value: &str) -> CoreResult<String> {
    let p = Path::new(value);
    if p.is_absolute() {
        return Err(CoreError::invalid("Sandbox path must be relative"));
    }
    let mut out = Vec::new();
    for c in p.components() {
        match c {
            Component::Normal(v) => out.push(v.to_string_lossy().to_string()),
            Component::CurDir => {}
            _ => {
                return Err(CoreError::invalid(
                    "Sandbox path escapes the application directory",
                ))
            }
        }
    }
    Ok(out.join("/"))
}
async fn resolve(
    t: &mut EmulatorTransport,
    package: &str,
    kind: &str,
    path: &str,
) -> CoreResult<(String, String)> {
    let roots = roots(t, package).await?;
    let root = roots
        .get(kind)
        .ok_or_else(|| CoreError::invalid(format!("Unknown sandbox root: {kind}")))?;
    let rel = relative(path)?;
    let requested = if rel.is_empty() {
        root.clone()
    } else {
        format!("{root}/{rel}")
    };
    let raw = t
        .exec(
            &format!(
                "test -e {} && test -e {} || exit 45; readlink -f {}; readlink -f {}",
                shell_quote(root),
                shell_quote(&requested),
                shell_quote(root),
                shell_quote(&requested)
            ),
            true,
        )
        .await
        .map_err(|_| {
            CoreError::new(
                audb_protocol::ErrorCode::NotFound,
                format!("Sandbox path not found: {kind}/{rel}"),
            )
        })?;
    let lines: Vec<_> = raw.lines().collect();
    if lines.len() != 2 {
        return Err(CoreError::new(
            audb_protocol::ErrorCode::NotFound,
            format!("Sandbox path not found: {kind}/{rel}"),
        ));
    }
    if lines[1] != lines[0] && !lines[1].starts_with(&format!("{}/", lines[0])) {
        return Err(CoreError::invalid(
            "Resolved sandbox path escapes the application directory",
        ));
    }
    Ok((lines[0].into(), lines[1].into()))
}
pub async fn clear(t: &mut EmulatorTransport, package: &str, confirm: bool) -> CoreResult<Value> {
    let discovered = paths(t, package).await?;
    let targets: Vec<Value> = discovered["paths"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|v| v["exists"] == true)
        .cloned()
        .collect();
    if !confirm {
        return Ok(json!({"package":package,"dryRun":true,"targets":targets,"removed":[]}));
    }
    let _ = app::stop(t, package).await;
    let stopped = app::wait(
        t,
        package,
        false,
        Duration::from_secs(5),
        Duration::from_millis(100),
    )
    .await?;
    if stopped["matched"] != true {
        return Err(CoreError::runtime(format!(
            "Application did not stop before clearing data: {package}"
        )));
    }
    let values: Vec<String> = targets
        .iter()
        .filter_map(|v| v["path"].as_str().map(shell_quote))
        .collect();
    if !values.is_empty() {
        t.exec(&format!("rm -rf -- {}", values.join(" ")), true)
            .await?;
    }
    Ok(
        json!({"package":package,"dryRun":false,"targets":targets,"removed":targets.iter().filter_map(|v|v["path"].as_str()).collect::<Vec<_>>()}),
    )
}
pub async fn list(
    t: &mut EmulatorTransport,
    package: &str,
    kind: &str,
    path: &str,
) -> CoreResult<Value> {
    let (root, target) = resolve(t, package, kind, path).await?;
    let q = shell_quote(&target);
    let raw=t.exec(&format!("for p in {q}/* {q}/.[!.]* {q}/..?*; do test -e \"$p\" || test -L \"$p\" || continue; stat -c '%F\t%s\t%Y\t%n' \"$p\"; done"),true).await?;
    Ok(Value::Array(parse_listing(&raw, &root)))
}

fn parse_listing(raw: &str, root: &str) -> Vec<Value> {
    raw.lines()
        .filter_map(|line| {
            let fields: Vec<_> = line.splitn(4, '\t').collect();
            if fields.len() != 4 {
                return None;
            }
            let name = Path::new(fields[3]).file_name()?.to_string_lossy();
            let relative = Path::new(fields[3])
                .strip_prefix(root)
                .ok()?
                .to_string_lossy()
                .trim_start_matches('/')
                .to_string();
            Some(json!({
                "name": name,
                "path": relative,
                "type": match fields[0] {
                    "directory" => "directory",
                    "regular file" => "file",
                    "symbolic link" => "symlink",
                    _ => "other",
                },
                "size": fields[1].parse::<u64>().ok()?,
                "modified": fields[2].parse::<f64>().ok()?,
            }))
        })
        .collect()
}
pub async fn pull(
    t: &mut EmulatorTransport,
    package: &str,
    kind: &str,
    path: &str,
) -> CoreResult<Vec<u8>> {
    let (_, remote) = resolve(t, package, kind, path).await?;
    t.download_bytes(Path::new(&remote)).await
}
pub async fn sqlite(
    t: &mut EmulatorTransport,
    package: &str,
    kind: &str,
    path: &str,
    query: &str,
) -> CoreResult<Value> {
    let normalized = query.trim_start().to_ascii_uppercase();
    if !["SELECT", "WITH", "PRAGMA", "EXPLAIN"]
        .iter()
        .any(|v| normalized.starts_with(v))
    {
        return Err(CoreError::invalid(
            "Only read-only SQLite queries are allowed",
        ));
    }
    let data = pull(t, package, kind, path).await?;
    let temp = tempfile::NamedTempFile::new()?;
    std::fs::write(temp.path(), data)?;
    let connection = Connection::open_with_flags(temp.path(), OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    let mut statement = connection
        .prepare(query)
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    let columns = statement
        .column_names()
        .iter()
        .map(|v| v.to_string())
        .collect::<Vec<_>>();
    let count = statement.column_count();
    let mut cursor = statement
        .query([])
        .map_err(|e| CoreError::runtime(e.to_string()))?;
    let mut rows = Vec::new();
    while let Some(row) = cursor
        .next()
        .map_err(|e| CoreError::runtime(e.to_string()))?
    {
        let mut values = Vec::new();
        for i in 0..count {
            values.push(
                match row
                    .get_ref(i)
                    .map_err(|e| CoreError::runtime(e.to_string()))?
                {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(v) => json!(v),
                    ValueRef::Real(v) => json!(v),
                    ValueRef::Text(v) => json!(String::from_utf8_lossy(v)),
                    ValueRef::Blob(v) => json!({"blobBytes":v.len()}),
                },
            );
        }
        rows.push(Value::Array(values));
        if rows.len() > 1000 {
            break;
        }
    }
    let truncated = rows.len() > 1000;
    rows.truncate(1000);
    Ok(
        json!({"package":package,"database":format!("{kind}/{path}"),"columns":columns,"rows":rows,"truncated":truncated}),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn rejects_escape() {
        assert!(relative("../secret").is_err());
        assert!(relative("/etc/passwd").is_err());
        assert_eq!(relative("a/./b").unwrap(), "a/b");
    }

    #[test]
    fn parses_tab_separated_stat_listing() {
        let values = parse_listing(
            "regular file\t42\t1234\t/home/defaultuser/data/check.db",
            "/home/defaultuser/data",
        );
        assert_eq!(values[0]["name"], "check.db");
        assert_eq!(values[0]["path"], "check.db");
        assert_eq!(values[0]["type"], "file");
        assert_eq!(values[0]["size"], 42);
    }
}
