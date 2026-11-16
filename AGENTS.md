# Agent instructions — `irix7/redox`

**Read [`bootstrap.md`](bootstrap.md) before your first change.** It is the map: the machine,
the toolchains, the emulator gate, the `bootp()redox` path, the 32 MB budget, the strip list
and the research in the sibling project.

The non-negotiables, repeated so they are hard to miss:

- This fork ports Redox to the **SGI Indy (IP24)**: MIPS III, n32, big-endian, **32 MiB**,
  booted from the PROM with `boot -f bootp()redox`.
- Upstream Redox OS does **not** accept LLM-generated contributions
  (`CONTRIBUTING.md`). All work stays in this fork; do not open upstream merge requests,
  issues or discussions.
- This fork is **MIT**. Do not copy GPL-3.0 code from `irix7/os` into it. Cite and reimplement.
- Every change that can affect memory reports before/after bytes in the commit message.
- Rust first; Australian English in prose; `git commit -s`.
- There are no deployed users. Replace internal interfaces outright rather than adding
  compatibility paths, migrations or version negotiation.
