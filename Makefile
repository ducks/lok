.PHONY: help version-bump release build test clean clippy

# Extract version from command line if passed as argument
# Supports: make release 0.2.0 OR make release VERSION=0.2.0
ifeq ($(VERSION),)
  VERSION := $(wordlist 2,$(words $(MAKECMDGOALS)),$(MAKECMDGOALS))
  $(eval $(VERSION):;@:)
endif

help:
	@echo "Lok Makefile"
	@echo ""
	@echo "Usage:"
	@echo "  make version-bump 0.2.0            - Bump version in Cargo.toml and commit"
	@echo "  make release 0.2.0                 - Bump version and push tag to trigger release"
	@echo "  make build                         - Build release binary"
	@echo "  make test                          - Run tests"
	@echo "  make clippy                        - Run clippy"
	@echo "  make clean                         - Clean build artifacts"
	@echo ""
	@echo "Examples:"
	@echo "  make version-bump 0.2.0            - Creates branch and bumps version"
	@echo "  make release 0.2.0                 - Full release (merge, tag, push)"
	@echo ""
	@echo "Note: VERSION prefix is optional:"
	@echo "  make release 20260124              - Creates tag v20260124"
	@echo "  make release v20260124             - Creates tag v20260124 (same)"

# Bump version in Cargo.toml and commit on a branch
version-bump:
ifeq ($(VERSION),)
	$(error No version specified. Use: make version-bump 0.2.0)
endif
	$(eval VERSION_CLEAN := $(patsubst v%,%,$(VERSION)))
	@echo "Creating release branch for version $(VERSION_CLEAN)..."
	@git checkout -b release/v$(VERSION_CLEAN)
	@echo "Bumping version to $(VERSION_CLEAN)..."
	@sed -i 's/^version = .*/version = "$(VERSION_CLEAN)"/' Cargo.toml
	@echo "Updating Cargo.lock..."
	@cargo check --quiet 2>/dev/null || true
	@git add Cargo.toml Cargo.lock
	@git commit -m "chore: bump version to $(VERSION_CLEAN)"
	@echo ""
	@echo "Created branch release/v$(VERSION_CLEAN)"
	@echo "Version bumped to $(VERSION_CLEAN)"
	@echo "Commit created"
	@echo ""
	@echo "To merge, tag, and push:"
	@echo "  make release $(VERSION)"

# Merge to main, tag, and push to trigger GitHub Actions release
release: version-bump
	$(eval VERSION_CLEAN := $(patsubst v%,%,$(VERSION)))
	@echo "Merging into main..."
	@git checkout main
	@git merge --no-ff release/v$(VERSION_CLEAN) -m "Merge branch 'release/v$(VERSION_CLEAN)'"
	@echo "Creating tag v$(VERSION_CLEAN) on main..."
	@git tag -a v$(VERSION_CLEAN) -m "Release v$(VERSION_CLEAN)"
	@echo "Pushing to origin..."
	@git push origin main
	@git push origin v$(VERSION_CLEAN)
	@echo ""
	@echo "Merged release/v$(VERSION_CLEAN) into main"
	@echo "Created tag v$(VERSION_CLEAN) on main"
	@echo "Pushed to main"
	@echo "Pushed tag v$(VERSION_CLEAN)"
	@echo "GitHub Actions will build release binaries"

# Build release binary
build:
	cargo build --release

# Run tests
test:
	cargo test

# Run clippy
clippy:
	cargo clippy -- -D warnings

# Clean build artifacts
clean:
	cargo clean
