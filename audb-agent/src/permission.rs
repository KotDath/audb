//! Typed Sailjail D-Bus and verified read/modify/write, serialized by the service.
use audb_protocol::{ErrorCode, PermissionAction};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{
    collections::{BTreeSet, HashMap},
    time::Duration,
};
use zbus::{
    blocking::{connection::Builder, Connection, Proxy},
    zvariant::OwnedValue,
};

const BUS: &str = "org.sailfishos.sailjaild1";
const PATH: &str = "/org/sailfishos/sailjaild1";
type Result<T> = std::result::Result<T, Failure>;
#[derive(Debug)]
pub struct Failure {
    pub code: ErrorCode,
    pub message: String,
    pub data: Option<Value>,
}
impl Failure {
    fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            data: None,
        }
    }
    pub fn response(self) -> Value {
        json!({"ok":false,"error":{"code":self.code,"message":self.message},"data":self.data})
    }
}
fn dbus_error(e: zbus::Error) -> Failure {
    let code = match &e {
        zbus::Error::MethodError(name, _, _) => match name.as_str() {
            "org.freedesktop.DBus.Error.AccessDenied" | "org.freedesktop.DBus.Error.AuthFailed" => {
                ErrorCode::PermissionDenied
            }
            "org.freedesktop.DBus.Error.UnknownMethod"
            | "org.freedesktop.DBus.Error.UnknownInterface" => ErrorCode::CapabilityUnavailable,
            "org.freedesktop.DBus.Error.ServiceUnknown"
            | "org.freedesktop.DBus.Error.NameHasNoOwner" => {
                ErrorCode::PermissionServiceUnavailable
            }
            "org.freedesktop.DBus.Error.InvalidArgs" => ErrorCode::InvalidArgument,
            _ => ErrorCode::OutcomeUnknown,
        },
        _ => ErrorCode::OutcomeUnknown,
    };
    Failure::new(code, e.to_string())
}
fn invalid(message: impl Into<String>) -> Failure {
    Failure::new(ErrorCode::InvalidArgument, message)
}
fn token(s: &str, limit: usize) -> bool {
    !s.is_empty()
        && s.len() <= limit
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
        && s != "."
        && s != ".."
}
pub fn validate(application: &str, action: &PermissionAction) -> Result<()> {
    if !token(application, 255) {
        return Err(invalid("Invalid Sailjail application ID"));
    }
    let permissions = match action {
        PermissionAction::Grant {
            permissions,
            all_requested,
            ..
        } => {
            if *all_requested != permissions.is_empty() {
                return Err(invalid("Specify permissions or --all-requested"));
            }
            permissions
        }
        PermissionAction::Revoke { permissions } => {
            if permissions.is_empty() {
                return Err(invalid("Specify permissions to revoke"));
            }
            permissions
        }
        _ => return Ok(()),
    };
    if permissions.len() > 256 || permissions.iter().any(|p| !token(p, 128)) {
        return Err(invalid("Invalid permission names or too many permissions"));
    }
    Ok(())
}
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct State {
    pub application_id: String,
    pub uid: u32,
    pub declared: Vec<String>,
    pub granted: Vec<String>,
    pub show_prompt: bool,
    pub mode: String,
    pub always_allowed: Option<bool>,
}
fn sorted(values: Vec<String>) -> Vec<String> {
    values
        .into_iter()
        .collect::<BTreeSet<_>>()
        .into_iter()
        .collect()
}
pub trait Store {
    fn read(&mut self, uid: u32, application: &str) -> Result<State>;
    fn set_prompt(&mut self, uid: u32, application: &str, enabled: bool) -> Result<()>;
    fn set_grants(&mut self, uid: u32, application: &str, permissions: &[String]) -> Result<()>;
}

fn failed(store: &mut impl Store, before: &State, phase: &str, mut error: Failure) -> Failure {
    let after = store.read(before.uid, &before.application_id);
    let (value, read_error) = match after {
        Ok(v) => (json!(v), Value::Null),
        Err(e) => (Value::Null, json!({"code":e.code,"message":e.message})),
    };
    error.data = Some(
        json!({"phase":phase,"before":before,"after":value,"afterError":read_error,"mayHaveChanged":true}),
    );
    error
}
pub fn execute(
    store: &mut impl Store,
    uid: u32,
    application: &str,
    action: &PermissionAction,
) -> Result<Value> {
    validate(application, action)?;
    let before = store.read(uid, application)?;
    if matches!(action, PermissionAction::List) {
        return Ok(json!(before));
    }
    let mut target = before.clone();
    let requested = match action {
        PermissionAction::Grant {
            permissions,
            all_requested,
            ..
        } => {
            if *all_requested {
                before.declared.clone()
            } else {
                sorted(permissions.clone())
            }
        }
        PermissionAction::Revoke { permissions } => sorted(permissions.clone()),
        _ => vec![],
    };
    if let Some(permission) = requested.iter().find(|p| !before.declared.contains(p)) {
        return Err(Failure::new(
            ErrorCode::PermissionNotDeclared,
            format!("{application} does not declare {permission}"),
        ));
    }
    match action {
        PermissionAction::Grant { disable_prompt, .. } => {
            if before.show_prompt && !disable_prompt {
                return Err(Failure::new(
                    ErrorCode::PromptEnabled,
                    "Permission dialog is enabled; add --disable-prompt to grant permissions",
                ));
            }
            target.show_prompt = false;
            target.granted = sorted(before.granted.iter().cloned().chain(requested).collect());
        }
        PermissionAction::Revoke { .. } => target.granted.retain(|p| !requested.contains(p)),
        PermissionAction::Reset => {
            target.show_prompt = true;
            target.granted.clear();
        }
        PermissionAction::Prompt { enabled } => {
            target.show_prompt = *enabled;
            if *enabled {
                target.granted.clear();
            }
        }
        PermissionAction::List => unreachable!(),
    }
    let changed = target.show_prompt != before.show_prompt || target.granted != before.granted;
    if target.show_prompt != before.show_prompt {
        store
            .set_prompt(uid, application, target.show_prompt)
            .map_err(|e| failed(store, &before, "set_prompt", e))?;
    }
    // Disabling a prompt auto-grants every declared permission; always rewrite
    // the intended set after toggling, even if target equals the original set.
    if changed || matches!(action, PermissionAction::Reset) {
        store
            .set_grants(uid, application, &target.granted)
            .map_err(|e| failed(store, &before, "set_grants", e))?;
    }
    let after = store
        .read(uid, application)
        .map_err(|e| failed(store, &before, "verify", e))?;
    if after.show_prompt != target.show_prompt || after.granted != target.granted {
        let mut error = Failure::new(
            ErrorCode::PermissionVerifyFailed,
            "Sailjail state differs from requested state; no action was repeated",
        );
        error.data = Some(
            json!({"phase":"verify","before":before,"after":after,"expected":target,"mayHaveChanged":changed}),
        );
        return Err(error);
    }
    Ok(
        json!({"applicationId":application,"uid":uid,"before":before,"after":after,"changed":changed,"restartMayBeRequired":changed}),
    )
}

pub struct Sailjail {
    connection: Connection,
    always_allowed: bool,
}
fn check_interface(xml: &str) -> Result<bool> {
    if xml.len() > 256 * 1024 {
        return Err(Failure::new(
            ErrorCode::CapabilityUnavailable,
            "Oversized Sailjail introspection",
        ));
    }
    let doc = roxmltree::Document::parse_with_options(
        xml,
        roxmltree::ParsingOptions {
            allow_dtd: true,
            ..Default::default()
        },
    )
    .map_err(|e| Failure::new(ErrorCode::CapabilityUnavailable, e.to_string()))?;
    let interface = doc
        .descendants()
        .find(|n| n.has_tag_name("interface") && n.attribute("name") == Some(BUS))
        .ok_or_else(|| {
            Failure::new(
                ErrorCode::CapabilityUnavailable,
                "Sailjail interface is missing",
            )
        })?;
    for (name, input, output) in [
        ("GetAppInfo", "s", "a{sv}"),
        ("GetGrantedPermissions", "us", "as"),
        ("SetGrantedPermissions", "usas", ""),
        ("GetShowPrompt", "us", "i"),
        ("SetShowPrompt", "usi", ""),
    ] {
        let method = interface
            .children()
            .find(|n| n.has_tag_name("method") && n.attribute("name") == Some(name))
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::CapabilityUnavailable,
                    format!("Aurora permission method {name} is missing"),
                )
            })?;
        for (direction, expected) in [("in", input), ("out", output)] {
            let signature: String = method
                .children()
                .filter(|n| {
                    n.has_tag_name("arg") && n.attribute("direction").unwrap_or("in") == direction
                })
                .filter_map(|n| n.attribute("type"))
                .collect();
            if signature != expected {
                return Err(Failure::new(
                    ErrorCode::CapabilityUnavailable,
                    format!("Unsupported {name} signature"),
                ));
            }
        }
    }
    Ok(interface.children().any(|n| {
        n.has_tag_name("method") && n.attribute("name") == Some("GetAlwaysAllowedApplications")
    }))
}
impl Sailjail {
    pub fn connect() -> Result<Self> {
        let connection = Builder::system()
            .map_err(dbus_error)?
            .method_timeout(Duration::from_secs(2))
            .build()
            .map_err(dbus_error)?;
        let proxy = Proxy::new(
            &connection,
            BUS,
            PATH,
            "org.freedesktop.DBus.Introspectable",
        )
        .map_err(dbus_error)?;
        let xml: String = proxy.call("Introspect", &()).map_err(dbus_error)?;
        let always_allowed = check_interface(&xml)?;
        Ok(Self {
            connection,
            always_allowed,
        })
    }
    fn proxy(&self) -> Result<Proxy<'_>> {
        Proxy::new(&self.connection, BUS, PATH, BUS).map_err(dbus_error)
    }
    pub fn capabilities(&self) -> Value {
        json!({"permissions":true,"backend":"sailjaild1","promptSemantics":"aurora-boolean","protocolVersion":crate::PROTOCOL})
    }
}
impl Store for Sailjail {
    fn read(&mut self, uid: u32, application: &str) -> Result<State> {
        let proxy = self.proxy()?;
        let info: HashMap<String, OwnedValue> =
            proxy.call("GetAppInfo", &(application,)).map_err(|e| {
                let mut error = dbus_error(e);
                if error.code == ErrorCode::InvalidArgument {
                    error.code = ErrorCode::AppNotFound;
                }
                error
            })?;
        let declared = info
            .get("Permissions")
            .and_then(|v| Vec::<String>::try_from(v.try_clone().ok()?).ok())
            .ok_or_else(|| {
                Failure::new(
                    ErrorCode::CapabilityUnavailable,
                    "Sailjail GetAppInfo does not contain a permission array",
                )
            })?;
        let mode = info
            .get("Mode")
            .and_then(|v| <&str>::try_from(v).ok())
            .unwrap_or("Unknown")
            .to_owned();
        let granted: Vec<String> = proxy
            .call("GetGrantedPermissions", &(uid, application))
            .map_err(dbus_error)?;
        let prompt: i32 = proxy
            .call("GetShowPrompt", &(uid, application))
            .map_err(dbus_error)?;
        if ![0, 1].contains(&prompt) {
            return Err(Failure::new(
                ErrorCode::CapabilityUnavailable,
                "Unsupported Aurora prompt state",
            ));
        }
        let always_allowed = if self.always_allowed {
            let apps: Vec<String> = proxy
                .call("GetAlwaysAllowedApplications", &())
                .map_err(dbus_error)?;
            Some(apps.iter().any(|v| v == application))
        } else {
            None
        };
        Ok(State {
            application_id: application.into(),
            uid,
            declared: sorted(declared),
            granted: sorted(granted),
            show_prompt: prompt == 1,
            mode,
            always_allowed,
        })
    }
    fn set_prompt(&mut self, uid: u32, application: &str, enabled: bool) -> Result<()> {
        self.proxy()?
            .call::<_, _, ()>("SetShowPrompt", &(uid, application, i32::from(enabled)))
            .map_err(dbus_error)
    }
    fn set_grants(&mut self, uid: u32, application: &str, permissions: &[String]) -> Result<()> {
        self.proxy()?
            .call::<_, _, ()>("SetGrantedPermissions", &(uid, application, permissions))
            .map_err(dbus_error)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    struct Aurora {
        state: State,
        writes: usize,
        fail_grants: bool,
        ignore_grants: bool,
    }
    impl Aurora {
        fn new() -> Self {
            Self {
                state: State {
                    application_id: "test-app".into(),
                    uid: 100123,
                    declared: vec!["DeviceInfo".into(), "UserDirs".into()],
                    granted: vec![],
                    show_prompt: true,
                    mode: "Normal".into(),
                    always_allowed: Some(false),
                },
                writes: 0,
                fail_grants: false,
                ignore_grants: false,
            }
        }
    }
    impl Store for Aurora {
        fn read(&mut self, _: u32, _: &str) -> Result<State> {
            Ok(self.state.clone())
        }
        fn set_prompt(&mut self, _: u32, _: &str, enabled: bool) -> Result<()> {
            self.writes += 1;
            if self.state.show_prompt != enabled {
                self.state.granted = if enabled {
                    vec![]
                } else {
                    self.state.declared.clone()
                };
            }
            self.state.show_prompt = enabled;
            Ok(())
        }
        fn set_grants(&mut self, _: u32, _: &str, permissions: &[String]) -> Result<()> {
            self.writes += 1;
            if self.fail_grants {
                return Err(Failure::new(
                    ErrorCode::OutcomeUnknown,
                    "Lost write response",
                ));
            }
            if !self.state.show_prompt && !self.ignore_grants {
                self.state.granted = sorted(
                    permissions
                        .iter()
                        .filter(|v| self.state.declared.contains(v))
                        .cloned()
                        .collect(),
                );
            }
            Ok(())
        }
    }
    fn run(store: &mut Aurora, action: PermissionAction) -> Result<Value> {
        execute(store, 100123, "test-app", &action)
    }
    fn grant(name: &str, disable: bool) -> PermissionAction {
        PermissionAction::Grant {
            permissions: vec![name.into()],
            all_requested: false,
            disable_prompt: disable,
        }
    }
    #[test]
    fn grant_requires_explicit_prompt_change_and_preserves_other_grants() {
        let mut store = Aurora::new();
        assert_eq!(
            run(&mut store, grant("UserDirs", false)).unwrap_err().code,
            ErrorCode::PromptEnabled
        );
        assert_eq!(store.writes, 0);
        run(&mut store, grant("UserDirs", true)).unwrap();
        assert_eq!(store.state.granted, vec!["UserDirs"]);
        let reply = run(&mut store, grant("DeviceInfo", false)).unwrap();
        assert_eq!(reply["after"]["granted"], json!(["DeviceInfo", "UserDirs"]));
        assert_eq!(reply["before"]["granted"], json!(["UserDirs"]));
        run(
            &mut store,
            PermissionAction::Revoke {
                permissions: vec!["UserDirs".into()],
            },
        )
        .unwrap();
        assert_eq!(store.state.granted, vec!["DeviceInfo"]);
    }
    #[test]
    fn disabling_prompt_alone_neutralizes_automatic_grants_and_reset_clears() {
        let mut store = Aurora::new();
        run(&mut store, PermissionAction::Prompt { enabled: false }).unwrap();
        assert!(!store.state.show_prompt);
        assert!(store.state.granted.is_empty());
        run(
            &mut store,
            PermissionAction::Grant {
                permissions: vec![],
                all_requested: true,
                disable_prompt: false,
            },
        )
        .unwrap();
        assert_eq!(store.state.granted, store.state.declared);
        run(&mut store, PermissionAction::Reset).unwrap();
        assert!(store.state.show_prompt);
        assert!(store.state.granted.is_empty());
    }
    #[test]
    fn enabling_prompt_reports_cleared_grants_and_idempotent_operations_skip_writes() {
        let mut store = Aurora::new();
        run(&mut store, grant("UserDirs", true)).unwrap();
        let count = store.writes;
        let result = run(&mut store, grant("UserDirs", false)).unwrap();
        assert_eq!(result["changed"], false);
        assert_eq!(store.writes, count);
        let result = run(&mut store, PermissionAction::Prompt { enabled: true }).unwrap();
        assert_eq!(result["before"]["granted"], json!(["UserDirs"]));
        assert_eq!(result["after"]["granted"], json!([]));
    }
    #[test]
    fn undeclared_permission_and_bad_input_never_write() {
        let mut store = Aurora::new();
        assert_eq!(
            run(&mut store, grant("Camera", true)).unwrap_err().code,
            ErrorCode::PermissionNotDeclared
        );
        assert_eq!(store.writes, 0);
        assert!(validate("test-app", &grant("UserDirs", false)).is_ok());
        assert!(validate("../escape", &grant("UserDirs", false)).is_err());
        assert!(validate("test-app", &grant("X;reboot", true)).is_err());
        assert!(validate(
            "test-app",
            &PermissionAction::Grant {
                permissions: vec![],
                all_requested: false,
                disable_prompt: true
            }
        )
        .is_err());
    }
    #[test]
    fn partial_write_failure_reports_real_after_without_retry() {
        let mut store = Aurora::new();
        store.fail_grants = true;
        let error = run(&mut store, grant("UserDirs", true)).unwrap_err();
        assert_eq!(error.code, ErrorCode::OutcomeUnknown);
        assert_eq!(store.writes, 2);
        let data = error.data.unwrap();
        assert_eq!(data["phase"], "set_grants");
        assert_eq!(data["before"]["showPrompt"], true);
        assert_eq!(data["after"]["granted"], json!(["DeviceInfo", "UserDirs"]));
    }
    #[test]
    fn successful_setter_is_not_success_without_readback() {
        let mut store = Aurora::new();
        store.ignore_grants = true;
        let error = run(&mut store, grant("UserDirs", true)).unwrap_err();
        assert_eq!(error.code, ErrorCode::PermissionVerifyFailed);
        assert_eq!(store.writes, 2);
    }
    #[test]
    fn aurora_signatures_are_checked_before_writing() {
        let xml = r#"<node><interface name="org.sailfishos.sailjaild1">
        <method name="GetAppInfo"><arg type="s"/><arg type="a{sv}" direction="out"/></method>
        <method name="GetGrantedPermissions"><arg type="u"/><arg type="s"/><arg type="as" direction="out"/></method>
        <method name="SetGrantedPermissions"><arg type="u"/><arg type="s"/><arg type="as"/></method>
        <method name="GetShowPrompt"><arg type="u"/><arg type="s"/><arg type="i" direction="out"/></method>
        <method name="SetShowPrompt"><arg type="u"/><arg type="s"/><arg type="i"/></method>
        </interface></node>"#;
        assert!(!check_interface(xml).unwrap());
        let with_dtd = format!("<!DOCTYPE node PUBLIC \"-//freedesktop//DTD D-BUS Object Introspection 1.0//EN\" \"http://www.freedesktop.org/standards/dbus/1.0/introspect.dtd\">{xml}");
        assert!(!check_interface(&with_dtd).unwrap());
        assert_eq!(
            check_interface(&xml.replace("GetShowPrompt", "GetLaunchAllowed"))
                .unwrap_err()
                .code,
            ErrorCode::CapabilityUnavailable
        );
        assert!(check_interface(&xml.replace("a{sv}", "s")).is_err());
    }
    #[test]
    fn permission_requests_work_without_display_and_cannot_override_uid() {
        let value = json!({"protocolVersion":crate::PROTOCOL,"action":{"command":"permission","application_id":"test-app","action":{"operation":"list"}}});
        let request: crate::Request = serde_json::from_value(value.clone()).unwrap();
        assert!(request.display.is_none());
        let mut bad = value;
        bad["action"]["uid"] = json!(0);
        assert!(serde_json::from_value::<crate::Request>(bad).is_err());
    }
}
