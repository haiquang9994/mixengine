# Runtime and package sourcing

MixEngine's hardest logistics problem is not code — it is having a trustworthy PHP 8.3 binary for six
OS/arch combinations, and keeping it current.

## The package index

A single signed `index.json`, published in its own repository and CDN-cached:

```json
{
  "schema": 1,
  "generated_at": "2026-08-10T00:00:00Z",
  "packages": [
    {
      "kind": "php", "version": "8.3.12", "channel": "stable",
      "artifacts": [
        { "os": "windows", "arch": "x86_64",
          "url": "https://cdn.mixengine.dev/php/8.3.12/php-8.3.12-win-x64.zip",
          "sha256": "…", "size": 31457280,
          "provides": ["php", "php-cgi", "php-fpm"] }
      ],
      "requires": { "vcredist": "2019" },
      "eol": "2027-12-31"
    }
  ]
}
```

- Signed with Ed25519 (minisign); the public key is compiled into the binary and rotated only via an
  app update.
- Every artifact is verified by SHA-256 *after* download; a mismatch deletes the file and fails loudly.
- The client caches the index for 6 hours and works offline against the cache.
- Old versions are never removed from the index — a blueprint pinning PHP 8.1.29 must keep working.

## Where binaries come from, per runtime

| Runtime | Windows | macOS | Linux |
| --- | --- | --- | --- |
| PHP | official windows.php.net builds (NTS + TS, VS-version matched) | **we build** (Homebrew's are prefix-bound) | **we build** static-ish against old glibc |
| Node.js | official nodejs.org tarballs/zips — usable as-is on all three | ditto | ditto |
| Python | `python-build-standalone` (relocatable, all platforms) | ditto | ditto |
| Ruby | **we build** | **we build** | **we build** |
| Caddy | official releases (single static binary) | ditto | ditto |
| Nginx | official Windows zip | **we build** | **we build** |
| MariaDB | official zip | official tarball | official tarball |
| PostgreSQL | EDB binaries zip | **we build** or EDB | EDB / **we build** |
| Redis | Microsoft's fork is dead → **we build** with MSVC, or ship Valkey | official source build | official source build |
| Memcached | **we build** | source build | source build |

"We build" means a reproducible build pipeline in the packaging repo, producing **relocatable**
artifacts. Relocatability is the requirement that breaks most upstream builds: hardcoded prefixes in
`php-config`, `pg_config`, RPATHs, and `.dylib` install names must be patched at build time or fixed
at install time.

## Relocation rules

- No absolute path baked into a binary or config. Paths come from arguments and generated config.
- macOS: `install_name_tool`/`@loader_path` for every bundled dylib; sign the result (Ventura+
  rejects modified signed binaries).
- Linux: `$ORIGIN` RPATH; bundle only what the distro will not reliably have.
- Windows: ship the required VC++ runtime check as a `requires` entry; prompt the user rather than
  installing it silently.
- After install, a **smoke test** runs the binary (`php -v`, `mariadbd --version`) and fails the
  install if it does not execute. Never register an artifact that has not been proven to run.

## Version policy

- Track upstream **supported** versions plus one year of EOL grace; mark EOL versions in the GUI with
  a warning but keep them installable.
- Security releases are pushed to the index promptly and surfaced as an update badge per runtime.
- Only stable channels are offered by default; RC/beta behind a setting.

## Size

Artifacts are compressed (zstd where we control the build). The GUI always shows the download size
before installing, and the storage screen shows what each installed version costs, with a one-click
"remove unused versions" that respects project pins.

## Offline and mirrors

- `MIXENGINE_INDEX_URL` and `MIXENGINE_MIRROR_URL` let a team host their own mirror; the signature
  requirement stays.
- `mix runtime install --from ./php-8.3.12.zip --sha256 …` for air-gapped machines.
