//! One-shot looks at the PipeWire graph: the playback and recording nodes, the default devices
//! the `default` metadata names, and changes to those defaults (what `wpctl set-default` does).

use anyhow::{Context, Result, bail};
use pipewire as pw;
use pw::{metadata::Metadata, types::ObjectType};
use std::{
    cell::{Cell, RefCell},
    collections::BTreeMap,
    rc::Rc,
};

/// The metadata key of the sink the session manager uses now, and of the source a user chose.
pub const DEFAULT_SINK: &str = "default.audio.sink";
pub const CONFIGURED_SOURCE: &str = "default.configured.audio.source";

/// A node in the graph.
#[derive(Clone, Debug, Default)]
pub struct Node {
    /// The registry id; only tests read it.
    #[allow(dead_code)]
    pub id: u32,
    pub props: BTreeMap<String, String>,
}
impl Node {
    pub fn prop(&self, key: &str) -> &str {
        self.props.get(key).map_or("", String::as_str)
    }
    /// `media.class`, e.g. `Audio/Sink`.
    pub fn class(&self) -> &str {
        self.prop("media.class")
    }
}

/// The graph at one moment.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub nodes: Vec<Node>,
    /// The `default` metadata's entries for the whole graph (subject 0), by key.
    pub defaults: BTreeMap<String, String>,
}
impl Snapshot {
    /// The node name a default key such as [`DEFAULT_SINK`] holds.
    pub fn default_name(&self, key: &str) -> Option<String> {
        name_from_json(self.defaults.get(key)?)
    }
}

/// The `name` in a `{"name":"..."}` metadata value.
pub fn name_from_json(value: &str) -> Option<String> {
    serde_json::from_str::<serde_json::Value>(value)
        .ok()?
        .get("name")?
        .as_str()
        .map(str::to_owned)
}

/// The metadata value that names `name`.
pub fn json_name(name: &str) -> String {
    serde_json::json!({ "name": name }).to_string()
}

/// A change to the defaults: set `key` to the node named `name`, or remove it.
pub struct Change<'a> {
    pub key: &'a str,
    pub name: Option<&'a str>,
}

/// Reads the graph and, when `change` is given, applies it first; the snapshot is taken after.
pub fn session(change: Option<Change<'_>>) -> Result<Snapshot> {
    pw::init();
    let mainloop = pw::main_loop::MainLoopRc::new(None).context("creating the PipeWire loop")?;
    let context = pw::context::ContextRc::new(&mainloop, None)?;
    let core = context
        .connect_rc(None)
        .context("no PipeWire daemon to connect to")?;
    let registry = core.get_registry().context("reading PipeWire's registry")?;
    let nodes = Rc::new(RefCell::new(Vec::<Node>::new()));
    let defaults = Rc::new(RefCell::new(BTreeMap::<String, String>::new()));
    let metadata = Rc::new(RefCell::new(
        None::<pw::registry::GlobalObject<pw::properties::PropertiesBox>>,
    ));
    let failed = Rc::new(RefCell::new(None::<String>));
    let pending = Rc::new(Cell::new(0));
    let done = Rc::new(Cell::new(false));
    let _core = core
        .add_listener_local()
        .done({
            let (pending, done, mainloop) = (pending.clone(), done.clone(), mainloop.clone());
            move |id, seq| {
                if id == pw::core::PW_ID_CORE && seq.seq() == pending.get() {
                    done.set(true);
                    mainloop.quit();
                }
            }
        })
        .error({
            let (failed, mainloop) = (failed.clone(), mainloop.clone());
            move |id, _seq, res, message| {
                if id == 0 {
                    *failed.borrow_mut() = Some(format!("{message} ({res})"));
                    mainloop.quit();
                }
            }
        })
        .register();
    let _registry = registry
        .add_listener_local()
        .global({
            let (nodes, metadata) = (nodes.clone(), metadata.clone());
            move |global| match global.type_ {
                ObjectType::Node => {
                    let Some(props) = global.props else { return };
                    nodes.borrow_mut().push(Node {
                        id: global.id,
                        props: props
                            .iter()
                            .map(|(k, v)| (k.to_owned(), v.to_owned()))
                            .collect(),
                    });
                }
                ObjectType::Metadata
                    if global
                        .props
                        .is_some_and(|p| p.get("metadata.name") == Some("default")) =>
                {
                    *metadata.borrow_mut() = Some(global.to_owned());
                }
                _ => {}
            }
        })
        .register();
    let round = |label: &str| -> Result<()> {
        done.set(false);
        pending.set(core.sync(0).context("syncing with PipeWire")?.seq());
        mainloop.run();
        if let Some(message) = failed.borrow().as_ref() {
            bail!("PipeWire connection lost while {label}: {message}");
        }
        if !done.get() {
            bail!("PipeWire went away while {label}");
        }
        Ok(())
    };
    round("listing devices")?;
    let global = metadata.borrow_mut().take();
    if let Some(global) = global {
        let proxy: Metadata = registry.bind(&global).context("binding the defaults")?;
        let _listener = proxy
            .add_listener_local()
            .property({
                let defaults = defaults.clone();
                move |subject, key, _type, value| {
                    if subject == 0
                        && let Some(key) = key
                    {
                        match value {
                            Some(value) => defaults.borrow_mut().insert(key.into(), value.into()),
                            None => defaults.borrow_mut().remove(key),
                        };
                    }
                    0
                }
            })
            .register();
        round("reading the defaults")?;
        if let Some(change) = change {
            match change.name {
                Some(name) => proxy.set_property(
                    0,
                    change.key,
                    Some("Spa:String:JSON"),
                    Some(&json_name(name)),
                ),
                None => proxy.set_property(0, change.key, None, None),
            }
            round("changing the defaults")?;
        }
    } else if change.is_some() {
        bail!("PipeWire has no default-device metadata (is a session manager running?)");
    }
    let nodes = nodes.take();
    let defaults = defaults.take();
    Ok(Snapshot { nodes, defaults })
}

/// The graph as it is now.
pub fn snapshot() -> Result<Snapshot> {
    session(None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_names_round_trip_through_json() {
        let value = json_name(r#"alsa_output."odd"#);
        assert_eq!(
            name_from_json(&value).as_deref(),
            Some(r#"alsa_output."odd"#)
        );
        assert_eq!(name_from_json("not json"), None);
        assert_eq!(name_from_json(r#"{"other":1}"#), None);
    }
}
