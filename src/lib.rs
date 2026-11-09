//! fullscreen-ws engine: on COSMIC, move a window to its own (empty) workspace when it
//! goes fullscreen, and move it back to where it came from when it leaves
//! fullscreen.
//!
//! Protocols used:
//!   ext_foreign_toplevel_list_v1   - list of windows
//!   zcosmic_toplevel_info_v1 (v3)  - per-window state / workspace / output
//!   zcosmic_toplevel_manager_v1    - move_to_ext_workspace, activate
//!   ext_workspace_manager_v1       - workspaces, groups, activate

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cosmic_protocols::toplevel_info::v1::client::{
    zcosmic_toplevel_handle_v1 as cth, zcosmic_toplevel_info_v1 as cti,
};
use cosmic_protocols::toplevel_management::v1::client::zcosmic_toplevel_manager_v1 as ctm;
use wayland_client::backend::ObjectId;
use wayland_client::protocol::{wl_output, wl_registry, wl_seat};
use wayland_client::{
    Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum, delegate_noop, event_created_child,
};
use wayland_protocols::ext::foreign_toplevel_list::v1::client::{
    ext_foreign_toplevel_handle_v1 as fth, ext_foreign_toplevel_list_v1 as ftl,
};
use wayland_protocols::ext::workspace::v1::client::{
    ext_workspace_group_handle_v1 as wgh, ext_workspace_handle_v1 as wsh,
    ext_workspace_manager_v1 as wsm,
};

const STATE_MAXIMIZED: u32 = 0;
const STATE_MINIMIZED: u32 = 1;
const STATE_FULLSCREEN: u32 = 3;

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { eprintln!("fullscreen-ws: {}", format!($($t)*)) };
}

#[derive(Default)]
struct Win {
    cosmic: Option<cth::ZcosmicToplevelHandleV1>,
    app_id: String,
    has_state: bool,
    fullscreen: bool, // the trigger: fullscreen, or maximized unless fullscreen_only
    raw_fullscreen: bool,
    raw_maximized: bool,
    minimized: bool,
    workspaces: Vec<ObjectId>,
    outputs: Vec<wl_output::WlOutput>,
    /// Fullscreen state at last evaluation; None = new window, treat as false.
    last_fullscreen: Option<bool>,
    /// Set while we have moved the window: (origin workspace, target workspace).
    moved: Option<(ObjectId, ObjectId)>,
    /// What to restore after a move: true = fullscreen, false = maximized.
    restore_fullscreen: bool,
    /// After a move COSMIC drops maximize/fullscreen; ignore that until here.
    settle_until: Option<Instant>,
    reapplied: bool,
    /// Window existed when we started; its first state is a baseline.
    preexisting: bool,
}

struct Ws {
    handle: wsh::ExtWorkspaceHandleV1,
    coords: Vec<u32>,
    hidden: bool,
    active: bool,
    name: String,
}

#[derive(Default)]
struct Group {
    workspaces: Vec<ObjectId>,
    outputs: Vec<wl_output::WlOutput>,
}

#[derive(Default)]
struct App {
    wins: HashMap<ObjectId, Win>, // keyed by ext_foreign_toplevel_handle id
    workspaces: HashMap<ObjectId, Ws>,
    groups: HashMap<ObjectId, Group>,
    info: Option<cti::ZcosmicToplevelInfoV1>,
    manager: Option<ctm::ZcosmicToplevelManagerV1>,
    ws_manager: Option<wsm::ExtWorkspaceManagerV1>,
    seat: Option<wl_seat::WlSeat>,
    ftl_global: Option<u32>,
    settings: Arc<Settings>,
    dirty: bool,
}

impl App {
    fn ws_name(&self, id: &ObjectId) -> String {
        self.workspaces.get(id).map(|w| w.name.clone()).unwrap_or_else(|| "?".into())
    }

    fn group_of(&self, ws: &ObjectId) -> Option<&Group> {
        self.groups.values().find(|g| g.workspaces.contains(ws))
    }

    fn windows_on(&self, ws: &ObjectId, except: &ObjectId) -> usize {
        self.wins
            .iter()
            .filter(|(k, w)| *k != except && !w.minimized && w.workspaces.contains(ws))
            .count()
    }

    fn evaluate(&mut self) {
        let debug = self.settings.debug.load(Ordering::Relaxed);
        let enabled = self.settings.enabled.load(Ordering::Relaxed);
        let fullscreen_only = self.settings.fullscreen_only.load(Ordering::Relaxed);
        for w in self.wins.values_mut() {
            w.fullscreen = w.raw_fullscreen || (!fullscreen_only && w.raw_maximized);
        }
        if debug {
            for w in self.wins.values() {
                log!(
                    "debug: {} fs={} min={} state={} ws={} out={}",
                    w.app_id, w.fullscreen, w.minimized, w.has_state,
                    w.workspaces.len(), w.outputs.len()
                );
            }
            for g in self.groups.values() {
                log!("debug: group ws={} out={}", g.workspaces.len(), g.outputs.len());
            }
        }
        let keys: Vec<ObjectId> = self.wins.keys().cloned().collect();
        for key in keys {
            let w = &self.wins[&key];
            if w.cosmic.is_none() || !w.has_state || w.workspaces.is_empty() {
                continue;
            }
            if w.preexisting && w.last_fullscreen.is_none() {
                let now = w.fullscreen;
                self.wins.get_mut(&key).unwrap().last_fullscreen = Some(now);
                continue;
            }
            let prev = w.last_fullscreen.unwrap_or(false);
            let now = w.fullscreen;
            if let Some(until) = w.settle_until {
                if now || Instant::now() >= until {
                    self.wins.get_mut(&key).unwrap().settle_until = None;
                } else {
                    // State was dropped by the move: put it back (once more), and
                    // don't treat it as the user leaving fullscreen.
                    let ready = w.moved.as_ref().is_some_and(|(_, t)| {
                        w.workspaces.contains(t)
                            && self.workspaces.get(t).is_some_and(|ws| ws.active)
                    });
                    if ready && !w.reapplied {
                        self.reapply(&key);
                        self.wins.get_mut(&key).unwrap().reapplied = true;
                    }
                    continue;
                }
            }
            if prev != now {
                log!("{}: fullscreen {} -> {}", self.wins[&key].app_id, prev, now);
            }
            if now && !prev && enabled {
                self.on_enter(&key);
            } else if !now && prev {
                self.on_leave(&key);
            }
            self.wins.get_mut(&key).unwrap().last_fullscreen = Some(now);
        }
    }

    fn on_enter(&mut self, key: &ObjectId) {
        let w = &self.wins[key];
        let Some(origin) = w
            .workspaces
            .iter()
            .find(|ws| self.group_of(ws).is_some())
            .cloned()
        else {
            log!("{}: fullscreen, but its workspace isn't in any known group", w.app_id);
            return;
        };
        let Some(group) = self.group_of(&origin) else {
            log!("{}: fullscreen, but its workspace isn't in any known group", w.app_id);
            return;
        };

        if self.windows_on(&origin, key) == 0 {
            log!("{}: fullscreen, already alone on its workspace", w.app_id);
            return;
        }

        // Pick the last empty, visible workspace in the same group.
        let target = group
            .workspaces
            .iter()
            .filter(|id| **id != origin)
            .filter(|id| self.workspaces.get(*id).is_some_and(|ws| !ws.hidden))
            .filter(|id| self.windows_on(id, key) == 0)
            .max_by_key(|id| {
                let pos = group.workspaces.iter().position(|x| x == *id);
                (self.workspaces[*id].coords.clone(), pos)
            })
            .cloned();
        let Some(target) = target else {
            log!("{}: fullscreen, but no empty workspace available", w.app_id);
            return;
        };
        let Some(output) = group.outputs.first().or(w.outputs.first()).cloned() else {
            log!("{}: fullscreen, but no output known", w.app_id);
            return;
        };

        log!(
            "{}: fullscreen -> moving from workspace {} to {} (window on {:?})",
            w.app_id,
            self.ws_name(&origin),
            self.ws_name(&target),
            w.workspaces.iter().map(|x| self.ws_name(x)).collect::<Vec<_>>()
        );
        let restore_fullscreen = w.raw_fullscreen;
        self.move_and_focus(key, &target, &output);
        let w = self.wins.get_mut(key).unwrap();
        w.moved = Some((origin, target));
        w.restore_fullscreen = restore_fullscreen;
        w.settle_until = Some(Instant::now() + Duration::from_millis(1500));
        w.reapplied = false;
        // Maximize is safe to restore right away. Fullscreen requests land on the
        // *active* workspace, so wait until the target is active (see evaluate).
        if !restore_fullscreen {
            self.reapply(key);
        }
    }

    fn reapply(&self, key: &ObjectId) {
        let w = &self.wins[key];
        let (Some(mgr), Some(cosmic)) = (&self.manager, &w.cosmic) else { return };
        if w.restore_fullscreen {
            mgr.set_fullscreen(cosmic, None);
        } else {
            mgr.set_maximized(cosmic);
        }
    }

    fn on_leave(&mut self, key: &ObjectId) {
        let Some((origin, target)) = self.wins.get_mut(key).unwrap().moved.take() else {
            return;
        };
        let w = &self.wins[key];
        // If the user moved it somewhere else themselves, leave it alone.
        if !w.workspaces.contains(&target) {
            return;
        }
        if !self.workspaces.contains_key(&origin) {
            log!("{}: origin workspace is gone, leaving window here", w.app_id);
            return;
        }
        let output = self
            .group_of(&origin)
            .and_then(|g| g.outputs.first())
            .or(w.outputs.first())
            .cloned();
        let Some(output) = output else { return };
        log!("{}: left fullscreen -> moving back to {}", w.app_id, self.ws_name(&origin));
        // Left fullscreen but still maximized: the move drops that, so restore it.
        let keep_maximized = w.raw_maximized;
        self.move_and_focus(key, &origin, &output);
        if keep_maximized
            && let (Some(mgr), Some(cosmic)) = (&self.manager, &self.wins[key].cosmic)
        {
            mgr.set_maximized(cosmic);
        }
    }

    fn move_and_focus(&self, key: &ObjectId, ws: &ObjectId, output: &wl_output::WlOutput) {
        let (Some(mgr), Some(wsm), Some(cosmic)) =
            (&self.manager, &self.ws_manager, &self.wins[key].cosmic)
        else {
            return;
        };
        let ws = &self.workspaces[ws].handle;
        mgr.move_to_ext_workspace(cosmic, ws, output);
        ws.activate();
        wsm.commit();
        if let Some(seat) = &self.seat {
            mgr.activate(cosmic, seat);
        }
    }
}

fn u32s(bytes: &[u8]) -> impl Iterator<Item = u32> + '_ {
    bytes
        .chunks_exact(4)
        .map(|c| u32::from_ne_bytes(c.try_into().unwrap()))
}

/// Behaviour switches; can be changed while the engine runs.
#[derive(Debug, Default)]
pub struct Settings {
    pub enabled: AtomicBool,
    pub fullscreen_only: AtomicBool,
    pub debug: AtomicBool,
}

impl Settings {
    pub fn new(enabled: bool, fullscreen_only: bool, debug: bool) -> Arc<Self> {
        Arc::new(Self {
            enabled: AtomicBool::new(enabled),
            fullscreen_only: AtomicBool::new(fullscreen_only),
            debug: AtomicBool::new(debug),
        })
    }
}

pub type Error = Box<dyn std::error::Error + Send + Sync>;

/// Take the per-user instance lock, blocking until it's free, so that only one
/// copy (the systemd service or the panel applet) acts on windows at a time.
/// Keep the returned file alive for as long as the engine runs.
pub fn acquire_instance_lock() -> Result<std::fs::File, Error> {
    acquire_instance_lock_with(|| log!("another instance is active, waiting for it to stop"))
}

/// Like [`acquire_instance_lock`], calling `on_wait` if it has to wait.
pub fn acquire_instance_lock_with(on_wait: impl FnOnce()) -> Result<std::fs::File, Error> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let path = std::path::Path::new(&dir).join("fullscreen-ws.lock");
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(path)?;
    if file.try_lock().is_err() {
        on_wait();
        file.lock()?;
    }
    Ok(file)
}

/// A connected, initialised watcher.
pub struct Engine {
    conn: Connection,
    queue: EventQueue<App>,
    app: App,
}

impl Engine {
    /// Bind the needed globals and load the current windows and workspaces.
    pub fn new(conn: Connection, settings: Arc<Settings>) -> Result<Self, Error> {
        let mut queue = conn.new_event_queue();
        let qh = queue.handle();
        let registry = conn.display().get_registry(&qh, ());
        let mut app = App { settings, ..Default::default() };
        queue.roundtrip(&mut app)?; // globals bound
        for (name, ok) in [
            ("zcosmic_toplevel_info_v1 (v3)", app.info.is_some()),
            ("zcosmic_toplevel_manager_v1 (v4)", app.manager.is_some()),
            ("ext_workspace_manager_v1", app.ws_manager.is_some()),
        ] {
            if !ok {
                return Err(format!("compositor does not provide {name}").into());
            }
        }
        // Bind the window list only after the workspace manager exists, so the
        // compositor can tell us which workspace each window is on.
        queue.roundtrip(&mut app)?; // workspaces announced
        let Some(ftl_name) = app.ftl_global else {
            return Err("compositor does not provide ext_foreign_toplevel_list_v1".into());
        };
        registry.bind::<ftl::ExtForeignToplevelListV1, _, _>(ftl_name, 1, &qh, ());
        queue.roundtrip(&mut app)?; // toplevels announced
        queue.roundtrip(&mut app)?;

        // Don't touch windows that were already fullscreen at startup. COSMIC may
        // send their initial state a little later, so take the first state we see
        // as the baseline.
        for w in app.wins.values_mut() {
            w.preexisting = true;
        }
        Ok(Self { conn, queue, app })
    }

    pub fn window_count(&self) -> usize {
        self.app.wins.len()
    }

    /// Print workspaces and windows (for `--list`).
    pub fn print_list(&mut self) -> Result<(), Error> {
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(50));
            self.queue.roundtrip(&mut self.app)?;
        }
        let app = &self.app;
        let mut ws: Vec<_> = app.workspaces.values().collect();
        ws.sort_by_key(|w| w.coords.clone());
        for w in ws {
            println!(
                "workspace {} coords={:?} active={} hidden={}",
                w.name, w.coords, w.active, w.hidden
            );
        }
        for w in app.wins.values() {
            let names: Vec<_> = w.workspaces.iter().map(|x| app.ws_name(x)).collect();
            println!(
                "window {} on {:?} fs={} max={}",
                w.app_id, names, w.raw_fullscreen, w.raw_maximized
            );
        }
        Ok(())
    }

    /// Debug helper: fullscreen the first window whose app id contains `needle`
    /// for two seconds. Note that COSMIC pulls the window onto the active
    /// workspace when fullscreen is requested this way.
    pub fn toggle_fullscreen(&mut self, needle: &str) -> Result<(), Error> {
        let needle = needle.to_lowercase();
        let cosmic = self
            .app
            .wins
            .values()
            .find(|w| w.app_id.to_lowercase().contains(&needle))
            .and_then(|w| w.cosmic.clone())
            .ok_or("no matching window")?;
        let mgr = self.app.manager.clone().ok_or("no toplevel manager")?;
        mgr.set_fullscreen(&cosmic, None);
        self.conn.flush()?;
        for _ in 0..20 {
            std::thread::sleep(Duration::from_millis(100));
            self.queue.roundtrip(&mut self.app)?;
        }
        mgr.unset_fullscreen(&cosmic);
        self.conn.flush()?;
        self.queue.roundtrip(&mut self.app)?;
        Ok(())
    }

    /// Move the first window whose app id contains `needle` to the workspace
    /// named `ws_name` (for `--move`).
    pub fn move_window(&mut self, needle: &str, ws_name: &str) -> Result<(), Error> {
        for _ in 0..5 {
            std::thread::sleep(Duration::from_millis(50));
            self.queue.roundtrip(&mut self.app)?;
        }
        let needle = needle.to_lowercase();
        let key = self
            .app
            .wins
            .iter()
            .find(|(_, w)| w.app_id.to_lowercase().contains(&needle))
            .map(|(k, _)| k.clone())
            .ok_or("no matching window")?;
        let ws = self
            .app
            .workspaces
            .iter()
            .find(|(_, w)| w.name == ws_name)
            .map(|(k, _)| k.clone())
            .ok_or("no workspace with that name")?;
        let output = self
            .app
            .group_of(&ws)
            .and_then(|g| g.outputs.first())
            .cloned()
            .ok_or("no output for that workspace")?;
        let (mgr, cosmic) = (
            self.app.manager.clone().ok_or("no toplevel manager")?,
            self.app.wins[&key].cosmic.clone().ok_or("window not ready")?,
        );
        mgr.move_to_ext_workspace(&cosmic, &self.app.workspaces[&ws].handle, &output);
        self.queue.roundtrip(&mut self.app)?;
        Ok(())
    }

    /// Watch for fullscreen changes forever (until the connection fails).
    pub fn run(mut self) -> Result<(), Error> {
        log!("watching {} windows", self.app.wins.len());
        loop {
            self.queue.blocking_dispatch(&mut self.app)?;
            if self.app.dirty {
                self.app.dirty = false;
                self.app.evaluate();
                self.conn.flush()?;
            }
        }
    }
}

// ---------------------------------------------------------------- dispatch --

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(
        app: &mut Self,
        reg: &wl_registry::WlRegistry,
        event: wl_registry::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        let wl_registry::Event::Global { name, interface, version } = event else { return };
        match interface.as_str() {
            "wl_output" => {
                reg.bind::<wl_output::WlOutput, _, _>(name, version.min(4), qh, ());
            }
            "wl_seat" if app.seat.is_none() => {
                app.seat = Some(reg.bind(name, version.min(1), qh, ()));
            }
            "ext_foreign_toplevel_list_v1" => app.ftl_global = Some(name),
            "zcosmic_toplevel_info_v1" if version >= 3 => {
                app.info = Some(reg.bind(name, 3, qh, ()));
            }
            "zcosmic_toplevel_manager_v1" if version >= 4 => {
                app.manager = Some(reg.bind(name, 4, qh, ()));
            }
            "ext_workspace_manager_v1" => {
                app.ws_manager = Some(reg.bind(name, 1, qh, ()));
            }
            _ => {}
        }
    }
}

delegate_noop!(App: ignore wl_output::WlOutput);
delegate_noop!(App: ignore wl_seat::WlSeat);
delegate_noop!(App: ignore ctm::ZcosmicToplevelManagerV1);

impl Dispatch<ftl::ExtForeignToplevelListV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &ftl::ExtForeignToplevelListV1,
        event: ftl::Event,
        _: &(),
        _: &Connection,
        qh: &QueueHandle<Self>,
    ) {
        if let ftl::Event::Toplevel { toplevel } = event {
            let id = toplevel.id();
            let cosmic = app
                .info
                .as_ref()
                .map(|info| info.get_cosmic_toplevel(&toplevel, qh, id.clone()));
            app.wins.insert(id, Win { cosmic, ..Default::default() });
        }
    }

    event_created_child!(App, ftl::ExtForeignToplevelListV1, [
        ftl::EVT_TOPLEVEL_OPCODE => (fth::ExtForeignToplevelHandleV1, ()),
    ]);
}

impl Dispatch<fth::ExtForeignToplevelHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        handle: &fth::ExtForeignToplevelHandleV1,
        event: fth::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id();
        match event {
            fth::Event::AppId { app_id } => {
                if let Some(w) = app.wins.get_mut(&id) {
                    w.app_id = app_id;
                }
            }
            fth::Event::Done => app.dirty = true,
            fth::Event::Closed => {
                if let Some(w) = app.wins.remove(&id) {
                    if let Some(c) = w.cosmic {
                        c.destroy();
                    }
                }
                handle.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<cti::ZcosmicToplevelInfoV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &cti::ZcosmicToplevelInfoV1,
        event: cti::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        if let cti::Event::Done = event {
            app.dirty = true;
        }
    }
}

impl Dispatch<cth::ZcosmicToplevelHandleV1, ObjectId> for App {
    fn event(
        app: &mut Self,
        _: &cth::ZcosmicToplevelHandleV1,
        event: cth::Event,
        key: &ObjectId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let Some(w) = app.wins.get_mut(key) else { return };
        match event {
            cth::Event::State { state } => {
                let s: Vec<u32> = u32s(&state).collect();
                if app.settings.debug.load(Ordering::Relaxed) {
                    log!("debug: raw state {} {:?}", w.app_id, s);
                }
                w.has_state = true;
                // Maximized counts too, unless FSWS_FULLSCREEN_ONLY is set.
                w.raw_fullscreen = s.contains(&STATE_FULLSCREEN);
                w.raw_maximized = s.contains(&STATE_MAXIMIZED);
                w.minimized = s.contains(&STATE_MINIMIZED);
            }
            cth::Event::OutputEnter { output } => w.outputs.push(output),
            cth::Event::OutputLeave { output } => w.outputs.retain(|o| o != &output),
            cth::Event::ExtWorkspaceEnter { workspace } => w.workspaces.push(workspace.id()),
            cth::Event::ExtWorkspaceLeave { workspace } => {
                let id = workspace.id();
                w.workspaces.retain(|x| x != &id);
            }
            _ => {}
        }
    }
}

impl Dispatch<wsm::ExtWorkspaceManagerV1, ()> for App {
    fn event(
        app: &mut Self,
        _: &wsm::ExtWorkspaceManagerV1,
        event: wsm::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wsm::Event::WorkspaceGroup { workspace_group } => {
                app.groups.insert(workspace_group.id(), Group::default());
            }
            wsm::Event::Workspace { workspace } => {
                app.workspaces.insert(
                    workspace.id(),
                    Ws { handle: workspace, coords: Vec::new(), hidden: false, active: false, name: String::new() },
                );
            }
            wsm::Event::Done => app.dirty = true,
            _ => {}
        }
    }

    event_created_child!(App, wsm::ExtWorkspaceManagerV1, [
        wsm::EVT_WORKSPACE_GROUP_OPCODE => (wgh::ExtWorkspaceGroupHandleV1, ()),
        wsm::EVT_WORKSPACE_OPCODE => (wsh::ExtWorkspaceHandleV1, ()),
    ]);
}

impl Dispatch<wgh::ExtWorkspaceGroupHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        handle: &wgh::ExtWorkspaceGroupHandleV1,
        event: wgh::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id();
        if let wgh::Event::Removed = event {
            app.groups.remove(&id);
            handle.destroy();
            return;
        }
        let Some(g) = app.groups.get_mut(&id) else { return };
        match event {
            wgh::Event::OutputEnter { output } => g.outputs.push(output),
            wgh::Event::OutputLeave { output } => g.outputs.retain(|o| o != &output),
            wgh::Event::WorkspaceEnter { workspace } => g.workspaces.push(workspace.id()),
            wgh::Event::WorkspaceLeave { workspace } => {
                let wid = workspace.id();
                g.workspaces.retain(|x| x != &wid);
            }
            _ => {}
        }
    }
}

impl Dispatch<wsh::ExtWorkspaceHandleV1, ()> for App {
    fn event(
        app: &mut Self,
        handle: &wsh::ExtWorkspaceHandleV1,
        event: wsh::Event,
        _: &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let id = handle.id();
        if let wsh::Event::Removed = event {
            app.workspaces.remove(&id);
            for w in app.wins.values_mut() {
                w.workspaces.retain(|x| x != &id);
            }
            for g in app.groups.values_mut() {
                g.workspaces.retain(|x| x != &id);
            }
            handle.destroy();
            return;
        }
        let Some(ws) = app.workspaces.get_mut(&id) else { return };
        match event {
            wsh::Event::Coordinates { coordinates } => ws.coords = u32s(&coordinates).collect(),
            wsh::Event::Name { name } => ws.name = name,
            wsh::Event::State { state } => {
                if let WEnum::Value(s) = state {
                    ws.hidden = s.contains(wsh::State::Hidden);
                    ws.active = s.contains(wsh::State::Active);
                }
            }
            _ => {}
        }
    }
}
