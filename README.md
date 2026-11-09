# cosmic-fullscreen-ws

Give fullscreen windows their own workspace on the [COSMIC](https://system76.com/cosmic) desktop.

When a window goes fullscreen or is maximized (F11, the maximize button, or
double-clicking the title bar), `fullscreen-ws` moves it to an empty workspace.
When it leaves fullscreen, it goes back to the workspace it came from.

- A window that is already alone on its workspace stays put.
- Windows already fullscreen when the service starts are left alone.
- If you move the window yourself while it's fullscreen, it isn't moved back.
- If the original workspace no longer exists, the window stays where it is.

It comes in two forms, and you can install either or both:

- **`fullscreen-ws`**: a tiny background service (systemd user unit). No UI.
- **`cosmic-ext-applet-fullscreen-ws`**: a panel applet with switches to turn it
  on/off and to choose whether maximized windows count. Heavier (it carries the
  libcosmic UI toolkit: ~20 MB binary, ~23 MB RAM vs ~3 MB for the service).

If both run, only one acts at a time (they share a lock in `$XDG_RUNTIME_DIR`).

This is an unofficial, third-party tool, not affiliated with System76.

## Install

### Fedora (COPR)

```sh
sudo dnf copr enable badman/cosmic-fullscreen-ws
sudo dnf install fullscreen-ws                      # the service
sudo dnf install cosmic-ext-applet-fullscreen-ws    # and/or the applet
```

For the applet, add it in **Settings → Desktop → Panel → Configure panel
applets → Add applet → Fullscreen Workspaces**.

The service is enabled for all users through a systemd preset and starts at the
next login. To start it right away:

```sh
systemctl --user start fullscreen-ws
```

### From source (any distro)

Requires Rust (stable, edition 2024).

```sh
cargo install --locked --git https://github.com/BrandonDoster/cosmic-fullscreen-ws
mkdir -p ~/.config/systemd/user
curl -fsSL https://raw.githubusercontent.com/BrandonDoster/cosmic-fullscreen-ws/main/packaging/fullscreen-ws.service \
  | sed "s|/usr/bin/fullscreen-ws|$HOME/.cargo/bin/fullscreen-ws|" \
  > ~/.config/systemd/user/fullscreen-ws.service
systemctl --user daemon-reload
systemctl --user enable --now fullscreen-ws
```

## Configuration

Settings are environment variables. Set them with `systemctl --user edit fullscreen-ws`:

```ini
[Service]
Environment=FSWS_FULLSCREEN_ONLY=1
```

| Variable | Effect |
|---|---|
| `FSWS_FULLSCREEN_ONLY=1` | React only to true fullscreen, not maximize |
| `FSWS_DEBUG=1` | Verbose logging |

## Usage

```sh
journalctl --user -u fullscreen-ws -f      # watch what it does
systemctl --user disable --now fullscreen-ws  # turn it off
fullscreen-ws --list                       # show workspaces and windows
```

## How it works

`fullscreen-ws` is a Wayland client that uses COSMIC's window and workspace
protocols:

| Protocol | Used for |
|---|---|
| `ext_foreign_toplevel_list_v1` | the list of windows |
| `zcosmic_toplevel_info_v1` (v3) | each window's state, workspace and output |
| `zcosmic_toplevel_manager_v1` (v4) | `move_to_ext_workspace`, re-applying maximize/fullscreen |
| `ext_workspace_manager_v1` | workspaces, and switching to the new one |

COSMIC's dynamic workspaces always keep an empty workspace at the end, and that
is where the window goes. COSMIC drops maximize/fullscreen when a window changes
workspace, so it is re-applied after the move.

## Building packages

```sh
packaging/make-srpm.sh fullscreen-ws                    # -> dist/fullscreen-ws-*.src.rpm
packaging/make-srpm.sh cosmic-ext-applet-fullscreen-ws  # -> dist/cosmic-ext-applet-*.src.rpm
rpmbuild --rebuild dist/<file>.src.rpm                  # optional local binary build
```

Each source RPM bundles the vendored Rust crates, so the RPM build itself runs
offline (as COPR's mock builds do).

## Releasing

Releases are built by `.github/workflows/release.yml` when a `v*` tag is pushed:
it builds both source RPMs, attaches them to a GitHub release, and submits them
to COPR.

1. Bump `version` in `Cargo.toml` and/or `applet/Cargo.toml`, and `Version:` plus
   a `%changelog` entry in the matching `packaging/*.spec`. The tag must equal both
   crate versions.
2. Commit, then `git tag -s v0.2.0 && git push --follow-tags`.

One-time COPR setup:

1. Create the project, e.g. `cosmic-fullscreen-ws`, with the Fedora chroots you
   want (x86_64 and aarch64).
2. Get an API token at <https://copr.fedorainfracloud.org/api/> and add its
   contents as the repository secret `COPR_CONFIG`.
3. Add the repository variable `COPR_PROJECT` = `<copr user>/cosmic-fullscreen-ws`.

`.copr/Makefile` also supports COPR's own "SCM / make srpm" builds, but only if
the repository is public.

## License

MIT
