use crate::features::config::{device_store::DeviceStore, state::DeviceState};
use crate::tools::shell_escape::escape_single_quote;
use crate::tools::ssh::SshClient;
use crate::tools::types::DeviceIdentifier;
use anyhow::{anyhow, Context, Result};

pub async fn execute(identifier: Option<String>, new_password: String) -> Result<()> {
    if new_password.is_empty() {
        return Err(anyhow!("New password cannot be empty"));
    }
    if new_password.contains(['\n', '\r']) {
        return Err(anyhow!("New password cannot contain line breaks"));
    }

    let identifier = match identifier {
        Some(identifier) => DeviceIdentifier::parse(&identifier),
        None => DeviceIdentifier::Name(DeviceState::get_current()?),
    };
    let mut device = DeviceStore::find(&identifier)?;

    let mut session = SshClient::connect(&device.host, device.port, &device.auth_path())
        .with_context(|| format!("Failed to connect to {}", device.display_name()))?;

    if device.root_password.is_empty() {
        SshClient::exec_as_devel_su(&mut session, "true", &new_password).with_context(|| {
            format!(
                "Failed to validate devel-su password on {}",
                device.display_name()
            )
        })?;
    } else {
        let command = format!(
            "busctl --system call org.nemo.passwordmanager /org/nemo/passwordmanager org.nemo.passwordmanager setPassword s '{}'",
            escape_single_quote(&new_password)
        );
        SshClient::exec_as_devel_su(&mut session, &command, &device.root_password).with_context(
            || {
                format!(
                    "Failed to change devel-su password on {} using cached rootPassword",
                    device.display_name()
                )
            },
        )?;
    }

    device.root_password = new_password;
    DeviceStore::update(device.clone())?;

    println!(
        "\x1b[1m\x1b[32msuccess\x1b[0m: Updated devel-su password for {}",
        device.display_name()
    );
    Ok(())
}
