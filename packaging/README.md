# Packaging

What turns a release build into the files a person downloads. One script per operating system, each
run on that system — there is no cross-packaging here, and CI's `build` job is three legs for that
reason.

Design: [`docs/superpowers/specs/2026-09-04-t85-installers-design.md`](../docs/superpowers/specs/2026-09-04-t85-installers-design.md).
Release process: [`.claude/operations/build-and-release.md`](../.claude/operations/build-and-release.md).

## Running it

```bash
bash packaging/windows/build.sh      # on Windows: a per-user installer and a portable zip
bash packaging/macos/build.sh        # on macOS:   one universal .pkg
bash packaging/linux/build-deb.sh    # on Linux:   .deb
bash packaging/linux/build-rpm.sh    #             .rpm
bash packaging/linux/build-appimage.sh  #          AppImage
```

Everything lands in `target/packaging/dist/`, with a `.sha256` beside each artifact. Each script
opens what it just made and checks the three binaries are in it before it exits — an empty archive
is a perfectly valid archive, and nothing else in the pipeline would notice.

| OS | Artifacts |
| --- | --- |
| Windows | `mixengine-<version>-windows-x86_64-setup.exe`, `mixengine-<version>-windows-x86_64.zip` |
| macOS | `mixengine-<version>-macos-universal.pkg` |
| Linux | `mixengine-<version>-linux-x86_64.AppImage`, `mixengine_<version>-1_amd64.deb`, `mixengine-<version>-1.x86_64.rpm` |

The version comes from `[workspace.package]` in the root `Cargo.toml`, so cutting a release is a
version bump and nothing else. Host architecture only: the second architecture on Windows and Linux
is roadmap task **T85a**, and macOS is universal here because Apple's toolchain builds the other
slice with no extra sysroot.

## What is not here

**Nothing signs anything.** Authenticode and an Apple Developer ID are not purchased
([ADR 0005](../.claude/decisions/0005-on-demand-elevation.md)), and the minisign signature that
actually protects an update is roadmap task **T86**. A `.sha256` is not a signature and is not
offered as one.

**No installer places `mixengine-elevate`.** MixEngine installs it itself, inside the elevation
prompt first-run setup already costs — [ADR 0015](../.claude/decisions/0015-the-helper-installs-itself.md).
The `.deb`, the `.rpm` and the `.pkg` ship it at that same path anyway, because they run as root and
can; the operation then finds its work already done. The per-user Windows installer, the portable zip
and the AppImage cannot, which is why the mechanism is not a packager's.

**No autostart entry.** `ServiceInstaller` is roadmap task **T85b**.
