# Built from an SRPM produced by packaging/make-srpm.sh, which bundles
# vendored crates so the build runs offline in mock.

# The release profile strips the binary, so there is no debuginfo to split out.
%global debug_package %{nil}
%global appid io.github.BrandonDoster.FullscreenWs

Name:           cosmic-ext-applet-fullscreen-ws
Version:        0.1.0
Release:        1%{?dist}
Summary:        COSMIC panel applet: give fullscreen windows their own workspace

License:        MIT
URL:            https://github.com/BrandonDoster/cosmic-fullscreen-ws
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

ExclusiveArch:  %{rust_arches}
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  make
BuildRequires:  pkgconfig(xkbcommon)
BuildRequires:  pkgconfig(wayland-client)
BuildRequires:  pkgconfig(dbus-1)
BuildRequires:  desktop-file-utils
Requires:       cosmic-panel

%description
A panel applet for the COSMIC desktop that moves fullscreen (and optionally
maximized) windows to an empty workspace of their own, and back again when
they leave fullscreen. Its popup has switches to turn this on or off and to
choose whether maximized windows count.

It runs the same engine as the fullscreen-ws service; if both are running,
only one acts at a time.

%prep
%autosetup -n %{name}-%{version}
cd applet
tar -xzf %{SOURCE1}
mkdir -p .cargo
cp vendor/cargo-config.toml .cargo/config.toml

%build
cd applet
export RUSTFLAGS="%{?build_rustflags}"
# Use the system's libintl (part of glibc) instead of building gettext.
export GETTEXT_SYSTEM=1
cargo build --release --frozen --offline

%install
install -Dpm0755 applet/target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm0644 applet/data/%{appid}.desktop %{buildroot}%{_datadir}/applications/%{appid}.desktop

%check
desktop-file-validate %{buildroot}%{_datadir}/applications/%{appid}.desktop

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_datadir}/applications/%{appid}.desktop

%changelog
* Mon Nov 09 2026 Brandon Doster <158637640+BrandonDoster@users.noreply.github.com> - 0.1.0-1
- Initial package
