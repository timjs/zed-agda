# Phase 3: optimisations

Phase 3 makes what phases 1 and 2 ([`PHASE1.md`](PHASE1.md),
[`PHASE2.md`](PHASE2.md)) built faster, and adds the goal commands that only
show information. Nothing of it is done yet.

## Subsumes the original phase 3

The original plan ([`PLAN.md`](PLAN.md), section 9.1) had a phase 3 called
"Commands": Agda's commands as Zed tasks (`tasks.json`), the
`agda-bridge client` with its socket, keymap snippets for Emacs and vim
leaders, and prompts in the terminal. This phase replaces it:

- The route through tasks was dropped after phase 0 ([`PHASE0.md`](PHASE0.md),
  "Decisions after phase 0"), because the language server can do the same
  with code actions, without a terminal. `languages/agda/tasks.json` and its
  keymap are gone.
- `agda-bridge client` and its socket stay, as a tool for debugging.
- What the tasks were for, Agda's commands with a typed expression, comes back
  below as code actions on goals, where the expression is the goal's text
  instead of a prompt.

## Planned

1. **A hover cache.** Hover on a goal asks Agda for the goal's type and
   context, and only when Agda is not busy, so during a long load it says
   "Agda is busy". The bridge could keep the last answer per goal, filled
   after every load, and show it, marked as possibly old, while Agda works.
2. **Goal commands that only show information**, as code actions on a goal,
   with the answer in the output file:
   - the goal's type together with the type of the expression in it
     (`Cmd_goal_type_context_infer`);
   - the normal form of the expression in it (`Cmd_compute`);
   - why a name is in scope (`Cmd_why_in_scope`);
   - the type of a helper function for the expression in it
     (`Cmd_helper_function`).
