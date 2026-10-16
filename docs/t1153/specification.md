# T1153: Nest Conduit search constraints (fix land/amend on orsay)

(Compact spec; no template in this repo. Task: https://orsay.hhh.pa/T1153.)

## Goal

Make Rust `arc` fully retrocompatible with PHP `arc` at
`ceb082ef` for every new-style Conduit `.search` call: constraints
nested under the top-level `constraints` wire param, exactly as
`ConduitSearchFuture::newFuture()` sends them
(`src/conduit/ConduitSearchFuture.php:96`:
`'constraints' => $constraints`). This un-breaks `arc land` and
`arc amend` against orsay.hhh.pa, which reject flat constraint
params with `ERR-CONDUIT-CALL`.

## Background

The Rust port copied the constraint arrays from the PHP hardpoint
queries but dropped the `ConduitSearchFuture` wrapper at some call
sites, sending e.g. `{"objectPHIDs": [...], "manual": false}` flat
instead of `{"constraints": {"objectPHIDs": [...], "manual": false}}`.
No Phabricator `.search` method takes these params flat, so every
such call fails on every server version.

## Scope — the 5 flat sites

| # | Method | Site | Fix |
|---|--------|------|-----|
| 1 | `harbormaster.buildable.search` | `land.rs` `fetch_build_views` | nest `{objectPHIDs, manual}` |
| 2 | `harbormaster.build.search` | `land.rs` `fetch_build_views` | nest `{buildables}` |
| 3 | `harbormaster.buildplan.search` | `land.rs` `fetch_plan_behaviors` | nest `{phids}` |
| 4 | `user.search` | `amend.rs` `query_user` | nest `{phids}` |
| 5 | `differential.revision.search` | `land.rs` `fetch_revision_views`, `RevisionSymbol::Phid` branch (`--revision <PHID>` land path) | nest `{phids}` |

Site 5 was missed by the task's audit and found by re-sweeping every
`.search` reference in `rust/src` against PHP
`ArcanistSimpleSymbolHardpointQuery`, which searches both `ids` and
`phids` via `newConduitSearch` (nested).

Audited clean (already nested, verified by reading each params
expression): `differential.revision.search` in `browse.rs`,
`diff_submit.rs` (`revision_search_params`), `revision.rs`
(`conduit_search`), `work.rs` (`search_params`, also used for
`maniphest.search`) and the `Id` branch of `fetch_revision_views`;
`differential.diff.search` (`diff_search_params`);
`diffusion.repository.search` (`revision.rs`, `look.rs`);
`file.search` (`download.rs`, `upload.rs`); `paste.search` x3.
Flat by design (PHP uses flat `yieldConduit`, verified in
`ArcanistRevisionParentRevisionsHardpointQuery.php`): `edge.search`.
Old-style flat APIs untouched: `differential.query`,
`differential.querydiffs`, `diffusion.querycommits`,
`differential.getcommitmessage`, `user.query`, `user.whoami`,
`maniphest.query`, and the rest of the method inventory.

## Fix design

1. Add one shared constructor in `rust/src/revision.rs` next to the
   existing `conduit_search` helper:
   `pub fn conduit_search_params(constraints: &Value, attachments: Option<&Value>) -> Value`,
   returning exactly `{"constraints": constraints}` plus
   `attachments` only when given — the PHP `ConduitSearchFuture`
   wire shape (`constraints`, optional `attachments`, `after`
   handled by the caller for pagination, as today).
2. Route all 5 sites through it. `fetch_revision_views` is
   restructured to build the `constraints` object in the
   `Id`/`Phid` match and wrap once, so a flat branch is
   unrepresentable there.
3. Wire shape stays byte-identical to PHP: no `attachments`/`after`
   keys where PHP sends none.

## Tests

- Update `land_tests.rs`
  `fetch_revision_views_queries_ids_and_phids`, which asserts the
  flat Phid payload, to the nested shape.
- Add param-capture assertions for the three land-gate queries
  (buildable/build/plan) and `amend::query_user`, asserting the
  exact nested payloads.
- Add a regression unit test enumerating every new-style `.search`
  call site in `rust/src` (method literals and `METHOD_*_SEARCH`
  constants, excluding `edge.search`, const definitions, tests,
  comments, and error strings) and asserting each site's params
  build carries a top-level `constraints` key (via the shared
  constructor, `conduit_search`, a `*_search_params` builder, or an
  adjacent `"constraints"` literal). Fails loudly on future flat
  additions.
- Constructor unit test for `conduit_search_params` (with and
  without attachments).

## Live verification (orsay.hhh.pa)

- The three land-gate queries + `user.search` succeed with nested
  params (no `ERR-CONDUIT-CALL`).
- `arc land --preview` proceeds past the online gates on a real
  revision (squash and merge strategies), without landing anything.

## Acceptance criteria

1. Given a revision with Harbormaster builds, when `arc land` runs,
   then the buildable/build/plan gate queries succeed (no
   `ERR-CONDUIT-CALL`) and the land completes.
2. Given any `.search` call site in the workspace, when its request
   params are built, then they contain a top-level `constraints`
   key (unit test).
3. `cargo fmt --check`, `cargo test`, `cargo clippy -- -D warnings`
   pass; no Jenkinsfile/.arclint changes needed (arcanist repo
   change only; phabricator repo untouched).

## Non-goals

orsay server upgrade; restoring PHP arc; changing old-style flat
APIs (`edge.search`, `*.query*`); touching the phabricator repo;
shipping the binary (normal release flow follows after review).
