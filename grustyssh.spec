Name:           wrustyssh
Version:        1.6.0
Release:        1%{?dist}
Summary:        A GTK4/libadwaita SSH client with tabbed terminals

License:        MIT
URL:            https://github.com/jwenzel2/wrustyssh
Source0:        %{name}-%{version}.tar.gz

BuildRequires:  rust >= 1.70
BuildRequires:  cargo
BuildRequires:  gcc
BuildRequires:  pkgconf-pkg-config
BuildRequires:  gtk4-devel
BuildRequires:  libadwaita-devel
BuildRequires:  vte291-gtk4-devel

Requires:       gtk4
Requires:       libadwaita
Requires:       vte291-gtk4

%description
wrustyssh is a GTK4/libadwaita SSH client written in Rust featuring
tabbed terminal emulation, connection profiles, SSH key management,
and port forwarding.

%prep
%autosetup

%build
cargo build --release

%install
install -Dm755 target/release/%{name} %{buildroot}%{_bindir}/%{name}
install -Dm644 data/%{name}.desktop %{buildroot}%{_datadir}/applications/%{name}.desktop
install -Dm644 icon.png %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/%{name}.png

%files
%{_bindir}/%{name}
%{_datadir}/applications/%{name}.desktop
%{_datadir}/icons/hicolor/256x256/apps/%{name}.png
