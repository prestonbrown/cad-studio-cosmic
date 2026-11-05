#!/bin/bash
# Publishes site/ to the orphan gh-pages branch of the GitHub repo.
#
# GitHub Pages is configured once, by the repo owner: Settings > Pages >
# Build and deployment > Deploy from a branch > gh-pages, folder / (root).
# The site is then served at https://prestonbrown.github.io/cad-studio-cosmic/.
#
#   scripts/site-publish.sh           dry run: shows what would be pushed
#   scripts/site-publish.sh --push    commits to gh-pages and pushes to origin
#
# Needs only git and the local clone; the dry run never touches the network.
# Refuses unless origin is github.com:prestonbrown/cad-studio-cosmic.
set -eu

mode=dry
case "${1:-}" in
    "") ;;
    --push) mode=push ;;
    *) echo "usage: $0 [--push]" >&2; exit 2 ;;
esac

log() { echo "$(date +%H:%M:%S) $*"; }

root=$(git rev-parse --show-toplevel)
cd "$root"
[ -f site/index.html ] || { log "no site/index.html under $root"; exit 1; }

origin=$(git config --get remote.origin.url || true)
case "$origin" in
    git@github.com:prestonbrown/cad-studio-cosmic|git@github.com:prestonbrown/cad-studio-cosmic.git|\
    https://github.com/prestonbrown/cad-studio-cosmic|https://github.com/prestonbrown/cad-studio-cosmic.git|\
    ssh://git@github.com/prestonbrown/cad-studio-cosmic|ssh://git@github.com/prestonbrown/cad-studio-cosmic.git) ;;
    *) log "refusing: origin is '${origin:-unset}', not github.com:prestonbrown/cad-studio-cosmic"; exit 1 ;;
esac

work=$(mktemp -d)
trap 'git worktree remove --force "$work/tree" 2>/dev/null || true; rm -rf "$work"' EXIT

# Start from the remote gh-pages when it exists (push mode only; the dry run
# stays offline), else from a local gh-pages, else from an orphan.
if [ "$mode" = push ] && git fetch origin gh-pages 2>/dev/null; then
    git worktree add --detach "$work/tree" FETCH_HEAD >/dev/null
elif git rev-parse --verify -q refs/heads/gh-pages >/dev/null; then
    git worktree add --detach "$work/tree" gh-pages >/dev/null
else
    git worktree add --detach "$work/tree" HEAD >/dev/null
    git -C "$work/tree" checkout --orphan gh-pages >/dev/null 2>&1
fi

cd "$work/tree"
git rm -rfq . 2>/dev/null || true
find . -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -R "$root/site/." .
touch .nojekyll
git add -A

if git diff --cached --quiet 2>/dev/null && git rev-parse --verify -q HEAD >/dev/null; then
    log "gh-pages already matches site/; nothing to publish"
    exit 0
fi

log "gh-pages would contain:"
git ls-files | sed 's/^/    /'

if [ "$mode" = dry ]; then
    log "dry run: not committing, not pushing (pass --push to publish)"
    exit 0
fi

src=$(git -C "$root" rev-parse --short HEAD)
git commit -q -m "site: publish site/ from $src"
git push origin HEAD:refs/heads/gh-pages
log "pushed gh-pages"
