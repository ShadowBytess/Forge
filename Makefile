.PHONY: all build release install uninstall test clippy fmt clean

PREFIX ?= /usr/local
BINDIR := $(DESTDIR)$(PREFIX)/bin
DATADIR := $(DESTDIR)$(PREFIX)/share

all: release

build:
	cargo build

release:
	cargo build --release

test:
	cargo test

clippy:
	cargo clippy --all-targets

fmt:
	cargo fmt
	cargo fmt --check

clean:
	cargo clean

install: release
	install -Dm755 target/release/forge $(BINDIR)/forge
	install -Dm644 packaging/forge.desktop $(DATADIR)/applications/forge.desktop
	install -Dm644 packaging/icons/hicolor/scalable/apps/forge.svg \
		$(DATADIR)/icons/hicolor/scalable/apps/forge.svg
	install -Dm644 docs/forge.1 $(DATADIR)/man/man1/forge.1
	install -Dm644 docs/examples/forge-config.toml \
		$(DATADIR)/doc/forge/examples/forge-config.toml
	-install -Dm644 README.md $(DATADIR)/doc/forge/README.md
	-gtk-update-icon-cache -qtf $(DATADIR)/icons/hicolor 2>/dev/null || true
	-update-desktop-database -q $(DATADIR)/applications 2>/dev/null || true

uninstall:
	rm -f $(BINDIR)/forge
	rm -f $(DATADIR)/applications/forge.desktop
	rm -f $(DATADIR)/icons/hicolor/scalable/apps/forge.svg
	rm -f $(DATADIR)/man/man1/forge.1
	rm -rf $(DATADIR)/doc/forge
