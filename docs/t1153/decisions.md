# T1153 compat decisions: when `arc` runs retrocompatibly, and when not

Companion to [specification.md](specification.md). Task: https://orsay.hhh.pa/T1153.
Reference pair: arcanist `ceb082ef` (2020-07-23) + phabricator `ce0dc9a2`
(2020-08-05) as deployed on orsay.hhh.pa. PHP arc is frozen (last commit
May 2023); orsay is the only pinned server.

## Framework

**Default: always retrocompatible.** Every finding from the audit is fixed
to the ceb behavior unconditionally — same wire protocol, same behavior,
same observable output. There is no compat mode, no `--compat` flag, and
no version switch, for three reasons:

1. One pinned server. orsay @ `ce0dc9a2` is the only target; no second
   server generation needs the divergent behavior.
2. Frozen reference. PHP arc has no "new" behavior to track; the
   divergences are port artifacts and bugs, not adaptations.
3. Test-matrix cost. Dual behavior would double golden/wire coverage for
   zero consumers. (Revisit only if a second server generation enters
   scope; the per-finding log below is the input such a mode would need.)

The only ways a finding does NOT end up running the ceb implementation:

**Rule 1 — Capability-driven (port PHP's own runtime decision).** Where
PHP itself branches at runtime on a server signal, the port branches on
the same signal. The "when" is the signal PHP reads, not a local mode:

| PHP decision | Server signal | Rust trigger |
|---|---|---|
| `revision.edit` vs legacy `createrevision` | `parsecommitmessage` returns `transactions` (absent pre-Oct-2017) | `xactions` non-empty after `pull_data_from_conduit` |
| `--draft` allowed on create | same as above | same; legacy branch errors with the ceb text |
| `edge.search` flat params | `edge.search` defines flat params, rejects `constraints` (probed) | always flat (verified live that nested is rejected) |
| legacy-query scalar types | orsay types `differential.query`/`user.query` scalars as strings (probed) | loose coercion accepts both (superset, servers agree) |

**Rule 2 — Keep-list (static, unconditional).** A finding keeps the Rust
behavior only if it meets a keep criterion AND is recorded below with
its rationale. Criteria: (a) PHP behavior is a crash, infinite loop, or
data-loss bug; (b) PHP behavior is unreachable dead code the port
sensibly replaced; (c) divergence has zero wire/observable effect on
orsay (encoding/ordering internals both sides accept). Keeps are
revisited never unless the criterion stops holding.

**Rule 3 — Unportable (static).** PHP-runtime-bound behavior (custom PHP
linter/engine classes, `phage` agent bootloader, PHP-`mt_rand` streams)
keeps the closest Rust equivalent, recorded below.

**Rule 4 — Not reproduced.** An audit claim that contradicts the code on
disk and live probes is recorded as clean with its evidence, plus a
regression test locking the correct behavior — never "fixed" by editing
correct code.

**Deciding a new finding:** reproduce against PHP @ `ceb082ef` first; if
it reproduces, fix to ceb unless it meets a Rule 2/3 criterion (record
why); if PHP itself branches on a server signal, apply Rule 1 with the
same signal; if it does not reproduce, apply Rule 4.

## Decision log

### Phase 0 (§1 search nesting) — all FIXED (Rule 0)

LAND-0a/b/c, AMEND-0, LAND-1 nest under `constraints` via the shared
`revision::conduit_search_params` constructor. `edge.search` stays flat
(Rule 1: server rejects nested — probed).

### Phase 1 (wire-critical)

| Finding | Decision | Trigger / note |
|---|---|---|
| R2 loose-int parser | FIXED (Rule 0) | `diff_submit::loose_i64/loose_u64` shared helper; applied to P6/P7/D15/P1 now, LAND-32/E26 in Phase 3 |
| PATCH-P6/P7 (string ids) | FIXED (Rule 0) | coerce at parse; orsay strings verified live |
| DIFF-D15 (`user.query` list) | FIXED (Rule 0) | iterate list rows like PHP `validateCommitMessage` |
| PATCH-P1 diffID type | KEEP BOTH (Rule 2c) | orsay accepts string AND number with identical bytes (probed); PHP never calls `getrawdiff`, so no reference shape exists |
| PATCH-P1 `parse_diff_rows` ids | FIXED via R2 (Rule 0) | string ids → `id: 0` → `ERR_NOT_FOUND` live today |
| PATCH-P2 (bundle from dicts) | FIXED (Rule 0) | render from `querydiffs` `changes[]` like `loadBundleFromConduit`, not `getrawdiff` text; verified byte-parity on D494 (480 files, 474 hunk headers identical to server raw) |
| DIFF-D1 (`edit:false`) | FIXED (Rule 0) | one-key wire fix + doc-cite correction |
| DIFF-D3/D5 + D2-create | FIXED (Rule 1) | create path pulls server fields+xactions via `parsecommitmessage`; `revision.edit` iff xactions non-empty (the signal PHP reads); legacy branch keeps the ceb `--draft` error |
| DIFF-D4 (git hunks) | FIXED (Rule 0) | git submits parse range diff text through `parse_unified_diff` like PHP `generateChanges` |
| EXPORT-E9 (encoding) | FIXED (Rule 0) | render converts UTF-8→target at the stdout sink (was reversed into mojibake); valid UTF-8 passes through untouched; cp1252-family via table, latin1-family 1:1, unknown throws. Former Rule 3 residual (lossy `String` vs raw bytes) CLOSED by R1 byte transport — see Residuals rows (keeps: submit `?`-replacements Rule 2a; carries: `--preview`, svn, unknown-encoding messages) |
| EXPORT-E10 (hunk corpus) | FIXED (Rule 0) | corpus is body-only (strip `@@` header) for bundle AND submit — one parser feeds both in PHP; render rebuilds headers from offsets incl. the small-hunk splitter port (`breakHunkIntoSmallHunks`), pinned by the live D494 `.arclint` vector |
| RB-D26 (tasks order) | FIXED via R8 (Rule 0) | `preserve_order` serde so PHID-keyed objects keep server order; follow-ups: signature encoder now sorts keys explicitly (it claimed to but relied on BTreeMap order), `.arclint` linter order is file order like PHP's `foreach` (test updated) |
| RB-D27 (exit codes) | CLEAN (Rule 4) | not reproduced: 4 live failure modes (transport/API/missing-token/outside-repo) all yield stderr + exit 1 on HEAD, matching the cited code; regression test added |

### Phase 2 Wave 1 (shared roots R3/R4/R5/R6/R9)

| Finding | Decision | Trigger / note |
|---|---|---|
| R3 LAND-16/BROWSE-B7/RB-D11/DL-D12/D13/UPLOAD-U16 (pagination) | FIXED (Rule 0) | new `conduit::search_pages` (`after`-cursor loop) + reuse of `revision::conduit_search`; repo/file match is last-wins like PHP `uri_map`/`ipull` overwrite |
| R4 error/exit envelope (LAND-37, CORE-6/21/49/53, LINT-1/2/3, UNIT-U3/4/5, LINTERS-L24, BROWSE-B20/22, DL-D2/D10, UPLOAD-U15, EXPORT-E7/E28, RB-D4) | FIXED (Rule 0) | one shared `conduit::{EnvelopeKind, ClassifiedError, report}` port of `scripts/arcanist.php:428-468` (subprocess) + `ArcanistRuntime.php:18-50` (toolset); dispatch per `ArcanistArcWorkflow` subclassing, proved at ceb |
| R4 DL-D2 / E7 / E28 / RB-D4-outside-repo exits | PRESENTATION ONLY (Rule 4 locks) | `didParseArguments` throw path is dead in toolset (exit unchanged); E7/E28 are subprocess exit 1 not 255; outside-repo `branches` exits 1 via the support gate; only banner/usage-dump bytes changed — locked by tests |
| R4 CORE-6 conduit Display | FIXED (Rule 0) | bare messages, no invented `transport/protocol` prefixes |
| R5 LAND-10/LAND-20/LINT-8/DIFF-D16/BROWSE-B3/RB-D22 (TTY + prompts) | FIXED (Rule 0) | `require_tty` echo-then-throw gates; `[y/N]` defaults; exact ceb banner/essay bytes |
| R5 CORE-39/CORE-40 | CLEAN (Rule 4, corrected Wave 3) | token prompt requires TTY (matches); the R5 note said the chmod prompt "defaults NO" — wrong: ceb `ArcanistConfigurationManager.php:198` passes `$default_no = false` = `[Y/n]` default YES (`src/console/format.php:11-12`); Rust `install_certificate.rs` prompts `[Y/n]` and chmods on accept — locking tests added |
| R6 LAND-13/UNIT-U13/CORE-12/33/34/38 (config merge) | FIXED (Rule 0) | one `get_config::load_merged` precedence (runtime `--config` > local > project > user > system > default) shared by toolset (`SourceList`) and classic (`getConfigFromAnySource`) callers |
| R9 BROWSE-B24/UPLOAD-U13/CORE-62/RB-D29c (no-browser text) | FIXED (Rule 0) | one shared constant set to the exact ceb bytes |
| R4 gaps: land cascade-retry rewrap; arclint JSON-decode proxy arm; bare-`arc` Missing arm | CARRIED | documented mis-routes (retry-exhausted→fatal vs usage; proxy errors under Usage arm; Missing arm keeps usage dump) — exit codes unaffected except none cited; land owner picks up the first in Wave 2 |
| R6 carry-overs: `which` source labels; unit engine-param reads; land `load_saved_prompts` path | CARRIED | unowned call sites adopt `merged_value` labels / full merge in Wave 2 (read-B, unit, LAND-14 owners) |

### Phase 2 Wave 2 (sections, 8 agents)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| LAND-2..43 (sans done/keep) + cascade-retry + LAND-43 experiment | FIXED (Rule 0/1) | batched search/query discovery; anchored `Differential Revision`; push/submit failures rewrap to usage/1 after cascade retry (Wave-1 gap closed). LAND-43 experiment DIVERGED → restored the ceb rebase-then-merge flow (reduce path force-lands trees ceb rejects); MERGE Attempting/DONE lines removed |
| LAND-22/23..28/42-markers | CARRIED | need conduit handshake caching (LAND-22) and markers/branches.rs work — Wave 3 |
| LINT-4..45 (sans done) | FIXED (Rule 0) | renderers, JSON/XML, patcher, per-linter ports; CLEAN+locked: LINT-21 (unportable `canRun`, Rule 3), LINT-37, LINT-45. Carry: LINT-23 (hg.rs), LINT-34/35 (linters.rs) |
| UNIT-U1..26 (sans done) | FIXED (Rule 0) | discovery/selection/render/JSON paths; engine params via full merge (Wave-1 follow-through). Carry: U16/U17 (unit_config.rs), U22 (main.rs global flags), U20 producer order, U21 non-ASCII escaping (conduit.rs), U2 phutil-progress half (unverifiable, Rule 3-ish) |
| LINTERS-L1..28, COVER-C2..16 | FIXED/KEPT | per finding vs ceb; keeps: L15/L16 (XHP ported-only, Rule 3), L23 (subclass N/A, Rule 3), C2 (experiment: PHP misattributes N+1, keep Rust N — Rule 2a), C3/C4/C10/C12. Carry: C1 (base-chain design), C13 (svn/hg, T1128) |
| BROWSE-B1..26, DL-D1..17 (sans done) | FIXED (Rule 0) | resolve/open/download paths; keeps: B2/B4/B16 + D11-locked (LAND-41/loop/race rationale, Rule 2a). Carry: B25 (which.rs), B21/B23 remainder (other commands' `--` + help) |
| UPLOAD-U1..16, PASTE-P1..18, EXPORT-E (render halves) | FIXED (Rule 0) | RefView/DONE/grammar/willUpload/browse flows; own toGitPatch/toUnifiedDiff port (git-apply oracle green). Keeps: U6/U7/U8/U11 (PHP ships corrupt uploads, Rule 2a), U12 (Rule 2c). Carry: E11..E24/E37 parser halves (diff_local.rs) |
| CORE-1..67 + RB-D1..39 (cited subset) | FIXED (Rule 0) | new argparse/prompts/shell_complete engines; liberate CORE-50 (rebuild status ignored, DONE always — stale tests updated at merge, verified vs ceb); which source labels (Wave-1 follow-through). Uncited IDs need Wave-3 verification (fix or lock) |
| CORE-18 (SIGINT routing) | UNPORTED (Rule 3) | no signal API in std; libc transitive-only, Cargo.toml frozen |
| DIFF-D6..21, PATCH-P3..13 (sans done) | FIXED (Rule 0) | D8 bare-git submits via detection; D9 raw message ride-through (ported into `collect_mode_payload` at merge); D11/D12 re-verified faithful. D2 verified CLEAN (no edit). Carry: DIFF-D22, DL-D11 |

### Wave 2 remediation (CI-driven)

| Finding | Decision | Trigger / note |
|---|---|---|
| RB-D5 `--help` shape vs release gate | BOTH (gate wins the token, finding wins the body) | `arc --help` leads with the historical `usage:` line (release smoke greps it) ahead of the full WORKFLOWS reference; `arc help` stays pure `renderHelp`. No PHP byte spec either way |
| `ARCANIST` classic export (`scripts/arcanist.php:229`) | FIXED (Rule 0, R2) | global flags moved from `set_var` (`unsafe` in edition 2024) to `globals` cells (zero `unsafe` per geiger policy); hook children now inherit `ARCANIST` via `proc_env::command` at all 78 spawn sites, with the resolved name exported for every command (the 16-classic gate was wrong — PHP putenvs post-`selectWorkflow()` for all). See Residuals rows |
| LINT-10 separator preservation | FIXED (Rule 0) | `normalize_lexical` returns dot-free paths byte-identical (a `PathBuf` rebuild converts `/`→`\` on Windows and broke `^externals/` routing + displays); `..` resolution unchanged |
| Complexity ratchet (28) + platform tests | FIXED | three agents split Wave-2's over-threshold functions (verified 0 violations via local rca replication); d7 identity, root-chmod, and 4 Windows failures fixed (hg CRLF tolerance, unix-gated stubs, drive-absolute bin, separator preservation) |

### Phase 2 Wave 3 (sections, 5 agents)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| coreA: LINT-23, UNIT-U16/U17/U21-escape/U22, CORE-32/41/42/43/44/65, LAND-22 | FIXED (Rule 0) | hg amend via bundled extension; PCRE include/exclude; lowercase `\uXXXX` escaping end-to-end (`conduit.rs`); global-flag consumption; VCS-root `--local`; `[Y/n]` chmod default YES (CORE-41 — corrects the R5 "defaults NO" note above); literal `~/.arcrc` text; ceb example host; status nesting; live stderr streaming; cached handshake identity. CORE-18 stays UNPORTED (Rule 3). Carry: UNIT-U21 order half (`unit.rs`), CORE-65 stdout residual |
| coreB: LAND-23..28/42, RB-D9..39 (cited), BROWSE-B25 | FIXED (Rule 0) | conduit-required grids (tokenless); hg `hgcm` pairs; nonzero-exit future throw; unchecked `name`/`node`; support-gate order; toolset parse texts; strict remote parsing; verbatim `--limit`; no owner/unassigned conflict; fetch-then-push per remote; PHP-truthy `isClosed`; first PHID-less ref kept. Follow-up `6d2784f8`: support gate runs ahead of conduit setup (PHP `getRepositoryAPI` order — Wave-3 order preempted it). Carry: RB-D33/35/36/37, RB-D15 remainder |
| lintunit: COVER-C1/C10/C11, UNIT-U19/U20, LINT-34/35 | FIXED/KEPT | full base-commit chain (`arc:` rules, upstream, scratch file, prompt); first-seen insertion order end-to-end; strict version regexes. C10 keep (Rule 2a: PHP fatals on the empty-tree base). Convention drift: citations lack the `T1153` prefix — Wave-4 instruction. Carry: C13 (svn/hg, T1128) |
| diff: EXPORT-E11..E24, E37 | FIXED (Rule 0) | `diff_local.rs` parser halves: block-header keys, hunk-count validation, CRLF/unterminated tails, filemode lines, git filename extraction, synthetic data, changeset loop, path-merge. Carry: E14/E11/E13/E20 wiring into submit paths + DIFF-D22 |
| brup: BROWSE-B21/B23 | FIXED (Rule 0) | `--` terminator gate + generated workflow help across export/paste/upload (classic vs toolset dispatch). Carry: B21/B23 remainder (other commands), RB-D5 shared help renderer |

### Phase 2 Wave 4 (sections, 4 agents + live probes)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| w4-grid: RB-D33/35/36/37, RB-D15 | FIXED (Rule 0) | trailing-only empty drop (mid-stream still throws); float `/2` ceiling; rowless grid draws `\n` not `Array`; `phabricator.example.com/D123` footer; PHID-less first-wins + live load audit. Core duplicated RB-D33/35/36 — grid kept at merge (ceb: mid-stream empty throws `ArcanistGitCommitGraphQuery.php:153-160`; core's `Array` comment wrong) |
| w4-help: R10 (8 cmds), RB-D5, B21/B23 pilots | FIXED/VERIFIED/KEPT | shared `help.rs` renderer (`usage:` lead kept, no PHP spec); browse central FIXED to ceb bytes, 7 VERIFIED; `--` gate + generated help for anoid/version/weld; markers help KEPT (2b, abstract at ceb). Carry: R10 other 28, B21/B23 other 23 (complex parsers; binary still gated globally) |
| w4-core: U21-order, CORE-47..65, full ID sweep | FIXED/VERIFIED/KEPT | U21 order VERIFIED+locked; CORE-65 live pipes; 37 workflows (`browse` into `ALL_WORKFLOWS`); liberate/anoid/version/browser COREs; all 67 CORE + 40 RB-D + 25 LINTERS + 16 COVER IDs cited; keeps re-verified (C2 `idx($blame,$line)` etc.); RB-D29a Rule 4 (no `--check` at ceb). Carry: RB-D16/17/18 (which prose, needs PHP oracle) |
| w4-diffland: E11/13/14/15/20/36, D22, LAND-22, COMMIT-C3 | FIXED (Rule 0) | hunk explicitness bits; filemodes→properties; shared CompanionFold (`old_path` Option); synthetic-hunk attach + pinned submit invocation; shared hunk_stats; lint/unit child dicts into `arc:lint`/`arc:unit`; cached handshake at amend/commit; best-effort post-commit nudge. §9 diff gaps VERIFIED. Carry: §9 `work` under hg, `oldPath` shapes (wide blast radius) |
| w4-live: 21 orsay probes (read-only) | 17 VERIFIED, 4 DIVERGED→Wave 5 | ping/nested search/attachments/tasks/list/browse/download-vs-PHP all match; D494 untouched. DL-1 (lowercase `f123`/PHID-FILE), DL-2/DL-3 (symbol-engine/no-argv texts): port lifted dead `didParseArguments` validation, live path is the symbol engine — owner `download.rs`. TASKS-1 unknown-flag wording (minor; scope vs locked convention TBD) |

### Phase 2 Wave 5 (last planned wave, 4 agents)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| w5-download: DL-1/2/3, TASKS-1 | FIXED (Rule 0) | `FileSymbol::{Id,Phid}` engine rules (`[Ff]?`, PHID→`file.search`); symbol-engine garbage text; live no-argv text; DL-D3 reconciled (dead `didParseArguments` vs live `runWorkflow`); tasks unknown-flag classic text per-command (toolset lock kept). New: tasks positional + download unknown-flag wordings (no IDs) |
| w5-r10: other 28 `--help`s | FIXED/VERIFIED (Rule 0) | all 28 through the shared renderer, byte-identical to `arc help <cmd>`; HELP_ENTRIES verified vs ceb, no entry changes |
| w5-b2123: 10 gates + 15 classic helps | FIXED/VERIFIED/KEPT | `--` gate + generated help for 10 toolset (alias/amend/bookmarks/branches/inspect/land/liberate/look/upgrade/work); 15 classic helps, B21-exempt VERIFIED; markers help KEPT (2b). Merge: help bytes identical to R10 (renderer `**name**` default), R10 kept, gates kept |
| w5-diffland2: WORK-HG, OLD-PATH | FIXED (Rule 0) | hg work engine (bookmark/branch flows, save/restore/shelve); ADD `oldPath` shapes routed per pipeline. CI: hermeticized hg tests (bare-`.hg`-is-operable; sh `{desc}` match fails on Win → fixture dispatch) |
| RB-D16/17/18 (which prose) | CARRIED (blocked) | needs a PHP-execution oracle; no PHP in this environment — single remaining carry |

### Phase 2 Wave 6 (oracle carry + unnamed divergences, 1 agent)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| TASKS-2 (tasks positional wording) | FIXED (Rule 0) | positionals with no `$more_key` hit classic `ArcanistWorkflow.php:887-895`: `Unrecognized argument '%s'. Try '%s'.` — first extra wins (`51d440b3`) |
| DL-4 (download unknown-flag wording) | FIXED (Rule 0) | toolset `PhutilArgumentParser.php:819-825` full-value text: `Argument "%s" is unrecognized. Use "%s"...`; post-`--` leftovers ride the wildcard into symbol validation (`51d440b3`) |
| RB-D16/17/18 (which prose) | FIXED (Rule 0) | full ceb `arc which` port (`cc29ecbf`, merged `f44f450c`): 3 wrapped prose sections (`ArcanistWhichWorkflow.php:57-261`), `repository.query` association + reason trail (`ArcanistWorkflow.php:1940-2044`, `:264-285`; `ArcanistGitAPI.php:545-557`), raw 16-char base + `getCommitSummary` + will-be-sent prose; staged printing; usage-error classification (RB-D4); winning-value base truthiness (RB-D38); arc-rs-only sections dropped. 26 live-oracle scenarios byte-identical (stdout+stderr+exit, ANSI + stripped). Oracle note: temp PHP lacks ext-posix so it emits ANSI on pipes (real PHP strips, like Rust); prompt paths in `which` never consult posix, so all 26 comparisons stand |
| COV-W6 (codecov patch/project) | FIXED (tests) | `f44f450c` failed `codecov/patch` (87.90% vs 92.75%) + `codecov/project` (-0.02%): the `run_text`/`run_show_base`/`run_json` wiring was never invoked. `1b26cfa8` (+270 `which_tests.rs`, merged `e612f19c`) covers every executable line → patch 97.70%, project +0.10%, both green. Methodology note: `DA:0` on comment/continuation/`else`-marker lines inside executed blocks is an LLVM gap-segment artifact — verified unactionable (straight-line neighbors hit); never chase it |

### Phase 2 Residuals (R1/R2/R3)

| Finding(s) | Decision | Trigger / note |
|---|---|---|
| R1 EXPORT-E9 (lossy corpus) | FIXED (Rule 0) | byte transport (`a571d31b`, merged `ff037617`): invalid bytes ride the `String` pipeline as `U+E000`+`U+E100+b` (valid UTF-8 verbatim, ASCII structure untouched), restored at every byte sink; sink converts like `convertNonUTF8Diff()` (`ArcanistBundle.php:493-498`) with `mb_convert_encoding()` `?`-grouping independently probed (`E9 80`→`?`, `ED A0 80`→`???`, `F0 28 8C 28`→`?(?(`, overlongs→`??`); `Bundle` carries `encoding`, unknown throws at apply (PHP throw-after-branch order); 60+ oracle differentials byte-identical; dead `decode_patch`/`local_diff` removed (`3efb100f`) |
| R1 keeps/carries | KEPT/CARRIED | submit path stays lossy (Rule 2a: PHP `json_encode` fails → server error, operation dies; Rust completes with `?`-replacements — the only working end-to-end behavior since Phabricator stores UTF-8 anyway); `changes.json` undecodable paths lossy (preserved, PHP fails hard — exotic); `patch --preview` lossy CARRIED (`Io.out` is `String`-only — byte-faithful preview needs a shared-harness change); svn paths lossy CARRIED (String-only sinks, no svn oracle); unknown-encoding MESSAGES (`Unknown encoding` vs PHP's `String conversion ... failed` / `Failed to convert a hunk`) pre-existing, follow-up |
| R2 `ARCANIST` inheritance | FIXED (Rule 0) | `proc_env::command` re-attaches `ARCANIST` at all 78 spawn sites (`260c332f`, merged `fd88380b`; zero direct `Command::new` left in shipped code). Value fix (`1276fda8`): the 16-classic gate was wrong — PHP putenvs the `selectWorkflow()`-resolved name for EVERY command (`arcanist.php:229`; `$command` is by-ref rewritten, not raw) — gate + classic set removed, locking test proves a non-classic name exports |
| R3 root/Windows sensitivity | VERIFIED (no code) | audit found the suite already robust: unreadable-file tests probe-then-skip privileged runners (`lint_tests`, `upload_tests`, `lint_text`), upgrade writability uses flag-based `readonly()` (true as root/Windows), exec-bit tests are `0o755` + `cfg`-gated per platform, Windows CI green |

### Keep-list (Rule 2, recorded early per task §10 step 4)

| Finding | Criterion | Rationale |
|---|---|---|
| LAND-29 (`--revision D123` works) | (a) | PHP `(int)` cast yields 0 — a crash-equivalent bug; Rust parses |
| LAND-30 (cascade per-set) | (a) | PHP `$set` loop bug lands the wrong set; Rust passes each set |
| LAND-41 (AMBIGUOUS hash) | (a) | PHP prints a leftover loop hash; Rust prints the actual commit |
| CORE-14 (scalar stdin error) | (a) | PHP TypeError fatal; Rust clean error, same exit class |
| CORE-24 (invalid alias) | (a) | PHP fatals (undefined constant); Rust warns and ignores |
| CORE-26/27 (alias list/delete, fail-fast) | (a/b) | PHP silent stubs / legacy fallthrough; Rust implements |
| CORE-29 (alias spawn failure) | (a) | PHP fatal; Rust clean error |
| CORE-35 (non-object config) | (a) | PHP downstream fatal; Rust upfront rejection |
| CORE-46/54 (prompt EOF) | (a) | PHP loops forever; Rust fails fast |
| RB-D38a (`commit <40hex>` line) | (a) | PHP misparses; Rust robust (more correct) |
| EXPORT-E34 (bundle temp cleanup) | (a) | PHP leaks temp on tar failure; Rust always cleans |
| CORE-45 (empty user/cert rejected) | (a) | PHP stores empty credentials; Rust rejects (hardening) |

### Pending Phase 3 decisions

- ~~**LAND-43**~~ decided Wave 2 (see row above): experiment DIVERGED →
  ceb rebase-then-merge restored; re-verified Wave 3 (no change).
- ~~**COVER-C2**~~ decided Wave 2 (see row above): keep Rust N (Rule 2a —
  PHP misattributes N+1); re-verified Wave 3 (no change).
- Every finding tagged "Rust better" not yet enumerated above is
  provisionally KEEP under criterion (a)/(b); each gets a log row when
  its phase lands.

### Unportable (Rule 3, standing)

`upgrade` network flow (no git checkout to pull in a static binary),
XHP134 product-name literals (needs libphutil runtime), custom PHP
linter/engine/rule classes, `phage` toolset, `mt_rand` streams
(`weld --seed` covers determinism), PHP-`mt_rand` joints.
