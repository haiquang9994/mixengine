#!/usr/bin/env bash
# The user handbook: build the site, regenerate the command reference, restamp the translations,
# and check that all of it is current.
#
# Roadmap task T90.
# Design: docs/superpowers/specs/2026-09-05-t90-the-documentation-site-design.md
#
#   bash packaging/docs.sh              build the site into target/site/
#   bash packaging/docs.sh --reference  regenerate docs/guide/en/cli.md from `mix` itself
#   bash packaging/docs.sh --restamp    rewrite every Vietnamese page's source_sha256
#   bash packaging/docs.sh --check      build into a temp dir, validate it, diff the reference
#
# **The site is not committed** and `--check` therefore diffs nothing but the reference — see the
# design's D8: `bindings/` is source code another repository compiles, and this is what a browser
# receives at a URL. What *is* committed is the Markdown corpus and the one generated page.
#
# `--restamp` is run **after** translating a page, never instead of it. All it records is that
# somebody looked; no machine here can check that a translation is right.

source "$(dirname "${BASH_SOURCE[0]}")/common.sh"

mode="build"
while [ $# -gt 0 ]; do
  case "$1" in
    --reference)
      mode="reference"
      shift
      ;;
    --restamp)
      mode="restamp"
      shift
      ;;
    --check)
      mode="check"
      shift
      ;;
    *)
      echo "unknown argument: $1" >&2
      exit 64
      ;;
  esac
done

mix_require cargo

guide="$MIX_ROOT/docs/guide"
committed_reference="$guide/en/cli.md"

# Build the whole site into $1.
mix_build_site() {
  cargo run --quiet -p mixengine-docs --example build-site -- "$1"
}

# Print the command reference as `mix` generates it.
#
# `--quiet` so that cargo's own progress does not end up inside a Markdown document, and a plain
# redirect so the bytes are the binary's — which is what makes `--check`'s `diff` meaningful.
mix_reference() {
  cargo run --quiet -p mixengine-cli -- docs --reference
}

# The pages the corpus holds, per locale. Sorted by `LC_ALL=C` so the count is the same on every
# machine rather than the same in every locale.
mix_slugs() {
  (cd "$guide/$1" && find . -name '*.md' | sed 's|^\./||; s|\.md$||' | LC_ALL=C sort)
}

case "$mode" in
  build)
    out="$MIX_ROOT/target/site"
    mix_build_site "$out"
    echo "wrote $(find "$out" -type f | wc -l | tr -d ' ') files into $out"
    ;;

  reference)
    # Through a temporary file, and this is not tidiness. A plain redirect truncates the page
    # *before* cargo runs, and the page is compiled into the binary being built — so the build then
    # fails on a front matter block the redirect just deleted. Generate first, move afterwards.
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT
    mix_reference >"$work/cli.md"
    mv "$work/cli.md" "$committed_reference"
    echo "wrote $(wc -l <"$committed_reference" | tr -d ' ') lines into $committed_reference"
    ;;

  restamp)
    changed=0
    for page in "$guide"/vi/*.md; do
      grep -q '^source_sha256 = ' "$page" || continue

      slug="$(basename "$page" .md)"
      english="$guide/en/$slug.md"
      test -f "$english" || {
        echo "$page is a translation of $english, which does not exist" >&2
        exit 1
      }

      want="$(sha256sum "$english" | cut -d' ' -f1)"
      have="$(sed -n 's/^source_sha256 = "\(.*\)"$/\1/p' "$page")"
      test "$want" != "$have" || continue

      # Over the one line, into a temporary file and back: an in-place edit that failed halfway
      # would leave a page with no front matter at all, which is a build error rather than a diff.
      sed "s|^source_sha256 = \".*\"$|source_sha256 = \"$want\"|" "$page" >"$page.restamped"
      mv "$page.restamped" "$page"
      echo "restamped $page against $slug.md"
      changed=$((changed + 1))
    done
    test "$changed" -ne 0 || echo "every translation already names the page it was made from"
    ;;

  check)
    # Into temporary directories rather than in place, and compared with `diff` rather than with
    # `git diff --exit-code`: a red job should be a message and not also a dirty checkout, and this
    # has to answer on a machine that unpacked a tarball rather than cloned one.
    work="$(mktemp -d)"
    trap 'rm -rf "$work"' EXIT

    site="$work/site"
    mix_build_site "$site"

    for file in index.html index.json llms.txt robots.txt sitemap.xml style.css .nojekyll; do
      test -f "$site/$file" || {
        echo "the site is missing $file" >&2
        exit 1
      }
    done

    for locale in en vi; do
      test -f "$site/$locale/index.html" || {
        echo "the site is missing $locale/index.html" >&2
        exit 1
      }
      test -f "$site/$locale/llms-full.txt" || {
        echo "the site is missing $locale/llms-full.txt" >&2
        exit 1
      }

      while read -r slug; do
        test -f "$site/$locale/$slug.md" || {
          echo "the site is missing $locale/$slug.md" >&2
          exit 1
        }
        test -f "$site/$locale/$slug/index.html" || {
          echo "the site is missing $locale/$slug/index.html" >&2
          exit 1
        }
      done < <(mix_slugs "$locale")
    done

    # **The no-JavaScript promise, measured rather than asserted.** The generator turns raw HTML
    # into text, so a `<script>` reaching a published file means that switch was flipped.
    if grep -rl '<script' "$site" >/dev/null 2>&1; then
      echo "the published site carries a script — the generator escapes raw HTML (T90, D9):" >&2
      grep -rl '<script' "$site" >&2
      exit 1
    fi

    mix_reference >"$work/cli.md"
    if ! diff "$committed_reference" "$work/cli.md"; then
      echo "" >&2
      echo "The committed command reference is not what mix generates." >&2
      echo "Run: bash packaging/docs.sh --reference — and commit what it writes." >&2
      exit 1
    fi

    echo "the site builds ($(find "$site" -type f | wc -l | tr -d ' ') files) and the command reference is current"
    ;;
esac
