//! Windows Firewall rules that keep Moonlight from reaching the host. The
//! installer allows the host on every network, but a block rule (left when
//! Windows' "allow access" prompt was dismissed, for instance on a network
//! marked public) overrides any allow rule.

use anyhow::Result;
use windows::{
    Win32::{
        NetworkManagement::WindowsFirewall::*,
        System::{Com::*, Ole::IEnumVARIANT, Variant::*},
    },
    core::Interface,
};

struct ComUninit(bool);
impl Drop for ComUninit {
    fn drop(&mut self) {
        if self.0 {
            // SAFETY: pairs with CoInitializeEx returning S_OK or S_FALSE on this thread.
            unsafe { CoUninitialize() };
        }
    }
}

fn expand_env_path(text: &str) -> String {
    use windows::Win32::System::Environment::ExpandEnvironmentStringsW;
    if !text.contains('%') {
        return text.to_owned();
    }
    let wide: Vec<u16> = text.encode_utf16().chain(Some(0)).collect();
    // SAFETY: `wide` is NUL-terminated; two-call buffer sizing reads only into `out`.
    unsafe {
        let len = ExpandEnvironmentStringsW(windows::core::PCWSTR(wide.as_ptr()), None);
        if len == 0 {
            return text.to_owned();
        }
        let mut out = vec![0u16; len as usize];
        let written =
            ExpandEnvironmentStringsW(windows::core::PCWSTR(wide.as_ptr()), Some(&mut out));
        if written == 0 || written as usize > out.len() {
            return text.to_owned();
        }
        String::from_utf16_lossy(&out[..written.saturating_sub(1) as usize])
    }
}

fn wildcard_field(value: &str) -> bool {
    let trimmed = value.trim();
    trimmed.is_empty() || trimmed == "*"
}

fn rule_applies(
    app_name: Option<&str>,
    local_ports: Option<&str>,
    remote_addresses: Option<&str>,
    action: NET_FW_ACTION,
    program_lower: &str,
) -> bool {
    let app_matches = match app_name.map(str::trim).filter(|s| !s.is_empty()) {
        None => true, // Empty ApplicationName matches all programs.
        Some(name) => expand_env_path(name).to_lowercase() == program_lower,
    };
    if !app_matches {
        return false;
    }
    let any_port = local_ports.is_none_or(wildcard_field);
    let any_remote = remote_addresses.is_none_or(wildcard_field);
    match action {
        NET_FW_ACTION_BLOCK => {
            // Broad block-all or any block targeting the host executable overrides allow rules.
            app_name.is_some_and(|n| !n.trim().is_empty()) || (any_port && any_remote)
        }
        NET_FW_ACTION_ALLOW => {
            // A generic wildcard allow without ports/remotes counts only when unrestricted;
            // program-scoped allow rules still require unrestricted ports/remotes or all.
            any_port && any_remote
        }
        _ => false,
    }
}

/// What Windows Firewall does to inbound traffic for `program` on the networks
/// the PC is on now: each enabled block rule naming it, or the absence of any
/// allow rule. Empty when nothing stands in the way or the firewall is off.
pub fn problems(program: &std::path::Path) -> Result<Vec<String>> {
    // SAFETY: COM is initialised on this thread (and uninitialised strictly when CoInitializeEx
    // returned S_OK or S_FALSE), and the enumerator yields VT_DISPATCH variants read before
    // VariantClear.
    unsafe {
        let init = CoInitializeEx(None, COINIT_MULTITHREADED);
        let _com = ComUninit(
            init == windows::Win32::Foundation::S_OK || init == windows::Win32::Foundation::S_FALSE,
        );
        let policy: INetFwPolicy2 = CoCreateInstance(&NetFwPolicy2, None, CLSCTX_INPROC_SERVER)?;
        let active = policy.CurrentProfileTypes()?;
        let mut enabled = false;
        let mut default_block = false;
        for profile in [
            NET_FW_PROFILE2_DOMAIN,
            NET_FW_PROFILE2_PRIVATE,
            NET_FW_PROFILE2_PUBLIC,
        ] {
            if active & profile.0 != 0 && policy.get_FirewallEnabled(profile)?.as_bool() {
                enabled = true;
                if policy.get_DefaultInboundAction(profile) == Ok(NET_FW_ACTION_BLOCK) {
                    default_block = true;
                }
            }
        }
        if !enabled {
            return Ok(vec![]);
        }
        let program = program.to_string_lossy().to_lowercase();
        let items: IEnumVARIANT = policy.Rules()?._NewEnum()?.cast()?;
        let (mut problems, mut allowed) = (Vec::new(), !default_block);
        loop {
            let mut item = [VARIANT::default()];
            let mut fetched = 0;
            if items.Next(&mut item, &mut fetched).is_err() || fetched == 0 {
                break;
            }
            let rule = item[0]
                .Anonymous
                .Anonymous
                .Anonymous
                .pdispVal
                .as_ref()
                .and_then(|dispatch| dispatch.cast::<INetFwRule>().ok());
            let _ = VariantClear(&mut item[0]);
            let Some(rule) = rule else {
                continue;
            };
            let active_rule = rule.Direction().is_ok_and(|d| d == NET_FW_RULE_DIR_IN)
                && rule.Enabled().is_ok_and(|e| e.as_bool())
                && rule.Profiles().is_ok_and(|p| p & active != 0);
            if !active_rule {
                continue;
            }
            let Ok(action) = rule.Action() else {
                continue;
            };
            let app_name = rule.ApplicationName().ok().map(|b| b.to_string());
            let local_ports = rule.LocalPorts().ok().map(|b| b.to_string());
            let remote_addrs = rule.RemoteAddresses().ok().map(|b| b.to_string());
            if !rule_applies(
                app_name.as_deref(),
                local_ports.as_deref(),
                remote_addrs.as_deref(),
                action,
                &program,
            ) {
                continue;
            }
            match action {
                NET_FW_ACTION_BLOCK => problems.push(format!(
                    "Windows Firewall rule \"{}\" blocks incoming connections to the host",
                    rule.Name().map(|n| n.to_string()).unwrap_or_default()
                )),
                NET_FW_ACTION_ALLOW => allowed = true,
                _ => {}
            }
        }
        if !allowed && problems.is_empty() {
            problems.push(
                "no Windows Firewall rule allows incoming connections to the host on this network"
                    .into(),
            );
        }
        Ok(problems)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn firewall_rule_matcher_handles_env_vars_block_all_and_port_restrictions() {
        let prog = r"c:\program files\butterpollo\butterpollo.exe";
        assert!(rule_applies(
            Some(r"%ProgramFiles%\Butterpollo\butterpollo.exe"),
            Some("*"),
            Some("*"),
            NET_FW_ACTION_ALLOW,
            prog,
        ));
        assert!(rule_applies(None, None, None, NET_FW_ACTION_BLOCK, prog,));
        assert!(!rule_applies(
            Some(prog),
            Some("80"),
            Some("*"),
            NET_FW_ACTION_ALLOW,
            prog,
        ));
        assert!(!rule_applies(
            Some(prog),
            Some("*"),
            Some("192.168.1.99"),
            NET_FW_ACTION_ALLOW,
            prog,
        ));
    }

    #[test]
    fn a_program_without_rules_is_reported_when_the_firewall_is_on() {
        let problems = super::problems(std::path::Path::new(
            r"C:\no\such\butterpollo-firewall-probe.exe",
        ))
        .unwrap();
        // Either the firewall is off on this network, or nothing allows the probe.
        assert!(problems.len() <= 1);
        let installed = std::path::Path::new(r"C:\Program Files\Butterpollo\butterpollo.exe");
        if installed.exists() {
            // The installer's rule allows the installed host.
            assert_eq!(super::problems(installed).unwrap(), Vec::<String>::new());
        }
        assert!(
            problems
                .iter()
                .all(|p| p.starts_with("no Windows Firewall rule"))
        );
    }
}
