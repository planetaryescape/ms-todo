# Kickoff prompt for the first build session

Paste this to the coding agent on the build machine, after `git clone https://github.com/planetaryescape/ms-todo`:

---

You're building ms-todo, a Rust terminal client for Microsoft To Do. It was fully planned in an earlier session you didn't see. Everything from that session is in this repo.

1. Read `AGENTS.md`, then `docs/blueprint/README.md` and every document it lists, in order. Then read `docs/research/`. Pay particular attention to `11-decision-log.md`, which explains what was rejected and why.
2. Clone `planetaryescape/mxr` and `planetaryescape/spotuify` into `/tmp/ms-todo-refs/`. For each path in `09-reuse-map.md`, check whether it changed since the SHA listed. Note anything relevant.
3. Phase 0: confirm with me that the Entra app is registered and where the client ID is stored (1Password). Then run every spike in `12-open-questions.md` against a throwaway list in my account. Record the evidence and conclusions in that file, and update any blueprint documents whose assumptions changed. Stop and report before phase 1, listing any results that change the design.
4. Then work through `10-roadmap.md` phase by phase. Each phase ends only when its completion check is observed through the real `ms-todo` binary. Commit using `type: description`.

Q1, Q2, Q4 and Q5 in `12-open-questions.md` are answered. Ask me about Q3, and any new product questions, when they come up. Don't guess.

---
