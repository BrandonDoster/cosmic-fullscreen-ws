# Built from an SRPM produced by .copr/Makefile (packaging/make-srpm.sh), which
# bundles vendored crates so the build runs offline in mock.

# The release profile strips the binary, so there is no debuginfo to split out.
%global debug_package %{nil}

Name:           fullscreen-ws
Version:        0.1.0
Release:        1%{?dist}
Summary:        Move fullscreen and maximized windows to their own COSMIC workspace

License:        MIT
URL:            https://github.com/BrandonDoster/cosmic-fullscreen-ws
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

ExclusiveArch:  %{rust_arches}
BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  systemd-rpm-macros
Requires:       cosmic-comp

%description
fullscreen-ws is a small background service for the COSMIC desktop. When a
window goes fullscreen (or is maximized), it moves the window to an empty
workspace of its own; when the window leaves fullscreen, it is moved back to
the workspace it came from.

%prep
%autosetup -n %{name}-%{version}
tar -xzf %{SOURCE1}
mkdir -p .cargo
cp vendor/cargo-config.toml .cargo/config.toml

%build
export RUSTFLAGS="%{?build_rustflags}"
cargo build --release --frozen --offline

%install
install -Dpm0755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dpm0644 packaging/%{name}.service %{buildroot}%{_userunitdir}/%{name}.service
install -Dpm0644 packaging/80-%{name}.preset %{buildroot}%{_userpresetdir}/80-%{name}.preset

%post
%systemd_user_post %{name}.service

%preun
%systemd_user_preun %{name}.service

%files
%license LICENSE
%doc README.md
%{_bindir}/%{name}
%{_userunitdir}/%{name}.service
%{_userpresetdir}/80-%{name}.preset

%changelog
* Mon Nov 09 2026 Brandon Doster <158637640+BrandonDoster@users.noreply.github.com> - 0.1.0-1
- Initial package
