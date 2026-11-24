//! Neural Rendering for an app: frame generation that the host fetches from
//! its authors' GitHub release on this PC, places in the game's folder, keeps
//! on the release Rubylight pins ([`PINNED`]) and removes again. It is ANR FG
//! by Apex Compute (<https://github.com/apex-compute/ANR>, no licence
//! published), a DLSS frame generation provider that OptiScaler (GPL-3.0)
//! loads in place of the game's own: the release's full package brings
//! OptiScaler with it, its add-on goes into an OptiScaler the user installed.
//! Rubylight never hosts, bundles or changes its files.
//!
//! Rubylight 3.0 installed AMDNR by 3zwr1 (an OptiScaler fork, GPL-3.0) for
//! neural upscaling instead. The host no longer offers it, but still removes
//! what it placed then: the [`LEGACY_MANIFEST`] in a game folder names it.
//!
//! This module and its submodules decide purely over values (with no Windows
//! or filesystem I/O): which release asset may be fetched, whether archives and
//! cached packages match their checksums, where each file goes, which DLL name
//! OptiScaler loads under, how OptiScaler.ini is changed and put back, what
//! refuses a game folder (anti-cheat, another mod, ACL policy), how app keys,
//! pending removals and switch jobs transition, and which GPU can run it. The
//! host's `neural` module downloads and moves the files.

mod archive;
mod eligibility;
mod gpu;
mod ini;
mod manifest;

pub use archive::{
    Archive, Asset, Cached, Download, GITHUB, MAX_ARCHIVE, MAX_SUMS, PACKAGE, PINNED, Package, Pin,
    PinnedFile, REPOSITORY, Release, SUMS, Switch, allowed_asset, asset, check_archives,
    check_whole, pinned, required, serves, sums, verify_download,
};
pub use eligibility::{
    ADMINISTRATORS, ANTI_CHEAT, GameCheck, Job, NO_DLSS, Occupant, Refusal, Removed, Rights,
    Security, StatusInput, anti_cheat, classify_non_game_exe, custom_providers,
    dlss_frame_generation, enable_preconditions, game_why, is_helper_program, is_optiscaler_binary,
    is_shared_folder_name, other_mod, proxy, security_verdict, should_run_game_check, status_json,
    vulkan_only, without_runtime,
};
pub use gpu::{
    AMD, Adapter, Capable, Gpu, MIN_NVIDIA_DEVICE, NVIDIA, Probe, VC_RUNTIME, capable, capable_any,
    needs_probe, offered, probe_gpus, runtime,
};
pub use ini::{INI_SECTION, INI_VALUES, IniRecord, ini_apply, ini_restore, opti_dir};
pub use manifest::{
    ALREADY_ON, DOWNLOAD_FAILED, FG_PROXIES, KEY, LEGACY_KEY, LEGACY_MANIFEST, MANIFEST, Manifest,
    Mode, Now, PLACED_BY, PROVIDER, PROVIDER_DLL, PROVIDER_RUNTIME, PROVIDER_SETTINGS, PROXIES,
    PROXY_SOURCE, Pending, Placed, Removal, SETTINGS, SETTINGS_FILE, app_has_files,
    app_install_dir, app_keep_key, app_key_value, app_legacy_exe, app_placed_exe, app_placed_exes,
    dropped_apps, entry_path, in_the_way, kept_text, logs, merge_pending, prepare_file_check,
    provider_dlssg, provider_files, removal, retain_pending_not_done, retained_manifest, settings,
    shown, stale_manifest_paths, target,
};

#[cfg(test)]
mod tests;
