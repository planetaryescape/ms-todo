# Kickoff prompt for the first build session

Paste this to the coding agent on the build machine, after `git clone https://github.com/planetaryescape/ms-todo`:

---

You're building ms-todo, a Rust terminal client for Microsoft To Do. It was fully planned in an earlier session you didn't see. Everything from that session is in this repo.

1. Read `AGENTS.md`, then `docs/blueprint/README.md` and every document it lists, in order. Then read `docs/research/`. Pay particular attention to `11-decision-log.md`, which explains what was rejected and why.
2. Clone `planetaryescape/mxr` and `planetaryescape/spotuify` into `/tmp/ms-todo-refs/`. For each path in `09-reuse-map.md`, check whether it changed since the SHA listed. Note anything relevant.
3. Phase 0 is done (2026-09-24). The Entra app is registered: read its client ID from `docs/setup/entra-app-registration.md` or the `ms-todo Entra app` item in 1Password's `Environment Variables` vault. The spike results are in `12-open-questions.md` and the evidence in `docs/research/spikes/`. Still open, and not blocking rung 1:
   - the phone halves of S7 (categories) and S11 (due dates and reminders), which I do on my phone;
   - the S4 deltaLink replay. The saved links are outside the repo, on the machine that ran the spikes;
   - product questions Q3 and Q6–Q12. Ask me when they come up.
   Start with foundation turn F1 of `10-roadmap.md`: install and sign in. Rung 1, the skateboard (see my tasks), comes next and adds a minimal daemon (D-032) that clients talk to only over the protocol (D-031), and reads straight from Graph until rung 3a (D-034).
4. Then climb `10-roadmap.md` one rung at a time. Every rung is a release: it ends only when its completion check is observed through the installed `ms-todo` binary, and its demo has been run. Commit using `type: description`.

Q1, Q2, Q4 and Q5 in `12-open-questions.md` are answered. Ask me about Q3, Q6–Q12, and any new product questions, when they come up. Don't guess.

---
