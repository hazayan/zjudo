zjudo
=====

Goal
----
zjudo provides an alternative path for loading a FreeBSD kernel from a
first-stage Linux boot. It is built for multi-stage boot experiments
where Linux is used only to hand off control to a FreeBSD kernel.

What It Does
------------
- Loads a FreeBSD kernel and selected modules from Linux.
- Builds and passes FreeBSD loader metadata (modulep, env, symbols).
- Supports optional early userland images (mfsroot) as preloaded modules.
- Collects system/boot context (SMAP, EFI table data) for handoff.
- Provides structured debug output to help validate the handoff path.

Status (So Far)
---------------
- End-to-end kexec handoff into FreeBSD is working in test runs.
- Module preloading and loader environment injection are stable.
- Early userland (mfsroot) boot path is functional for bring-up tasks.
- Extensive logging exists for tracing boot parameters and module layout.

Build
-----
Use the Makefile targets:

- `make build` (default)
- `make release`
- `make test`

See `Makefile` for additional static and target-specific builds.

Layout
------
- `src/` core loader, module, and handoff logic
- `scripts/` helper scripts used in boot flows
- `dev.log` append-only development log with recent milestones

Notes
-----
zjudo is focused on the kernel handoff path itself. It intentionally keeps
the Linux stage minimal and treats it as a transient launcher into FreeBSD.
