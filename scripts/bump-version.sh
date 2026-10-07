#!/bin/bash
set -e

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT_DIR="$SCRIPT_DIR/.."
CARGO_TOML="$ROOT_DIR/Cargo.toml"

# Get current version from Cargo.toml
CURRENT_VERSION=$(cargo metadata --no-deps --format-version 1 --manifest-path "$CARGO_TOML" \
    | jq -r '.packages[] | select(.name == "replane") | .version')
echo "Current version: $CURRENT_VERSION"

# Parse version numbers
MAJOR=$(echo "$CURRENT_VERSION" | cut -d. -f1)
MINOR=$(echo "$CURRENT_VERSION" | cut -d. -f2)
PATCH=$(echo "$CURRENT_VERSION" | cut -d. -f3)

# Ask user what to bump
echo ""
echo "What do you want to bump?"
echo "  1) major ($MAJOR.$MINOR.$PATCH -> $((MAJOR + 1)).0.0)"
echo "  2) minor ($MAJOR.$MINOR.$PATCH -> $MAJOR.$((MINOR + 1)).0)"
echo "  3) patch ($MAJOR.$MINOR.$PATCH -> $MAJOR.$MINOR.$((PATCH + 1))) [default]"
echo ""
read -p "Enter choice [1-3, default=3]: " CHOICE

# Default to patch if empty
CHOICE=${CHOICE:-3}

case $CHOICE in
    1)
        MAJOR=$((MAJOR + 1))
        MINOR=0
        PATCH=0
        ;;
    2)
        MINOR=$((MINOR + 1))
        PATCH=0
        ;;
    3)
        PATCH=$((PATCH + 1))
        ;;
    *)
        echo "Invalid choice"
        exit 1
        ;;
esac

NEW_VERSION="$MAJOR.$MINOR.$PATCH"
NEW_TAG="v$NEW_VERSION"
echo ""
echo "New version: $NEW_VERSION"
read -p "Update Cargo.toml, commit, tag, and push? [y/N]: " CONFIRM

if [[ "$CONFIRM" =~ ^[Yy]$ ]]; then
    # Update the first `version = "..."` line, which belongs to [package]
    perl -0pi -e "s/^version = \"\Q$CURRENT_VERSION\E\"/version = \"$NEW_VERSION\"/m" "$CARGO_TOML"
    (cd "$ROOT_DIR" && cargo update --workspace --offline --quiet)
    echo "Updated Cargo.toml and Cargo.lock to version $NEW_VERSION"

    # Commit the change
    git -C "$ROOT_DIR" add Cargo.toml Cargo.lock
    git -C "$ROOT_DIR" commit -m "chore: bump version to $NEW_VERSION"
    echo "Committed version bump"

    # Create and push tag
    git -C "$ROOT_DIR" tag "$NEW_TAG"
    echo "Created tag $NEW_TAG"

    git -C "$ROOT_DIR" push origin HEAD "$NEW_TAG"
    echo "Pushed changes and tag $NEW_TAG to origin"

    echo ""
    echo "Done! Version $NEW_VERSION will be published by CI."
else
    echo "Aborted."
    exit 0
fi
