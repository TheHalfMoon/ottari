# Himsat Agent Instructions

## Language

Repository-facing technical content, code, comments, plans, specifications, tasks, reports, evidence, commit messages, PR bodies, reviewer responses, and governance artifacts must be written in English.

## Canonical authority

Read in this order before changing the repository:

1. `specs/CURRENT.md`
2. `.specify/memory/constitution.md`
3. `docs/execution-master-plan.md`
4. the active specification's `spec.md`, `plan.md`, and `tasks.md`
5. `governance/provenance/source-use-authorization.md`
6. referenced ADR/research/security/donor/qualification documents
7. live GitHub state, exact branch/head, diff, checks, reviews, and mergeability

Live repository and GitHub truth override stale plans, chat summaries, cached CI, stale hashes, stale donor assumptions, and remembered authority.

## SpecGrain discipline

Use SpecGrain's model as the planning and delivery control discipline:

- refine large objectives recursively;
- keep one independently understandable outcome per leaf;
- make `scope_in` and `scope_out` explicit;
- declare dependencies instead of relying on narrative order;
- define acceptance and evidence before implementation;
- include risk and recovery plans;
- bound context and change surface;
- do not call a candidate a Grain merely because it is small;
- do not claim native SpecGrain lifecycle state unless the actual SpecGrain tooling/state proves it.

The relevant upstream methodology repository is `TheHalfMoon/SpecGrain`. Himsat planning may adopt its concepts without copying its product code into Himsat unless a separately authorized bounded adoption later chooses to do so.

## Diffcipline discipline

Use Diffcipline's behavioral contract at the finish line:

**Think → Challenge → Minimize → Change → Prove**

Rules:

- `NOT RUN` is never `PASS`.
- Agent self-report is not verification evidence.
- Exact diff and exact revision matter.
- Minimality is subordinate to correctness, security, accessibility, data integrity, and compatibility.
- Risk determines rigor.
- Public performance/quality/superiority claims require reproducible evidence.
- Do not claim complete, verified, merged, qualified, secure, private, offline, or benchmark-superior without evidence that supports that exact claim.

The relevant upstream methodology repository is `TheHalfMoon/Diffcipline`.

## Product invariants

Do not weaken these without an explicit constitutional/governance change:

- no mandatory Himsat-hosted cloud for core product behavior;
- no mandatory account for local core behavior;
- no hidden fallback from local AI to remote AI;
- no telemetry SDK in default core binaries unless a future specification explicitly changes the constitution;
- user-visible, permission-respecting recording only;
- no attempts to bypass OS microphone/screen indicators or protected-content capture policies;
- evidence-linked factual intelligence;
- local encryption and user-controlled keys;
- exportable user-owned data;
- platform capability detection instead of unsupported promises;
- donor/model/dataset provenance before adoption.

## Donor/source code

The founder/user has recorded project-level source-use permission for every external source referenced anywhere in the repository as of the snapshot named in `governance/provenance/source-use-authorization.md`.

Do not ignore that authorization merely because an older planning note assumed no additional permission. For covered sources, `COPY`, `ADAPT`, `DEPEND`, `VENDOR`, and `REFERENCE` are all eligible strategies when technically appropriate and when active specification authority permits them.

Permission is not automatic adoption. Before non-trivial reuse:

1. identify repository and immutable revision/tag;
2. identify exact source path(s);
3. verify the exact path's public license/terms and, when public terms are insufficient, record the separate permission basis and exact scope needed for the intended Himsat use;
4. inspect submodules, generated/vendor code, and transitive dependencies separately;
5. inspect model/weight/data/font/asset licenses or separate permission separately from code permission;
6. preserve required notices/attribution and other obligations;
7. create/update a machine-readable provenance record as required by the canonical adoption boundary;
8. explain why copy/adaptation/dependency/vendor/native implementation is the best bounded engineering choice;
9. write Himsat-owned behavior tests;
10. keep material outside the proven permission/provenance scope out of the Himsat dependency/adoption graph.

Do not bulk-copy donors merely because reuse is permitted. Prefer the smallest reliable source reuse that improves product quality and long-term maintainability.

Earlier categorical exclusions based only on assumed lack of permission are superseded for covered sources. However, restrictive/public-license terms, historical license changes, enterprise subtrees, proprietary code, and unavailable source still require exact-path permission/provenance evidence before copying. Permission does not create source bytes that Himsat cannot access.

Current `xberg-io/xberg`, historical Kreuzberg lines, Screenpipe, MinerU, Marker variants, Anarlog `enterprise/**`, Superwhisper/OpenSuperWhisper, and every other covered source must therefore be evaluated on exact live source, exact permission scope, embedded third-party provenance, architecture, and tests rather than on stale blanket assumptions.

## Security rules

Treat captured audio, transcripts, speaker embeddings, screenshots, documents, memory graph data, and connector credentials as sensitive.

- never log plaintext secrets, credentials, vault keys, raw speaker embeddings, or full private transcripts by default;
- use OS secure storage for key material where available;
- plugins and agents receive explicit capabilities, not ambient authority;
- external writes require explicit policy and normally user approval;
- connector egress is content-minimized and auditable;
- model and plugin packages require hashes/signatures/manifests before execution;
- archive/document importers are untrusted-input parsers and require traversal/bomb/resource-limit defenses;
- media decoders and parsers require fuzz/adversarial testing proportional to risk.

## Platform truth

Desktop, iOS/iPadOS/watchOS, and Android differ materially. Do not generalize one platform's audio behavior to another.

Examples of known constraints at planning time:

- iOS background microphone recording requires the correct audio session/background configuration and remains subject to interruptions and OS policy;
- Apple recording intents require visible system recording state/Live Activity behavior where specified by the OS;
- Android microphone background capture requires a foreground service and modern Android restricts starting microphone foreground services from the background;
- Android third-party apps generally cannot capture cellular call uplink/downlink without privileged permissions;
- Android playback capture depends on MediaProjection permission and the source app's capture policy;
- newer ScreenCaptureKit capabilities must be version-gated; beta/future APIs are not baseline requirements.

## Branching and merge discipline

- no force-push or shared-history rewriting unless repository governance is explicitly changed;
- prefer bounded feature/planning branches;
- re-read canonical `main` after merges;
- verify exact head, scope, CI/checks, reviews/threads, and mergeability immediately before merge;
- use expected-head protection where supported;
- unavailable, skipped, neutral, billing-blocked, or absent review systems are not PASS;
- preserve residual risks and blockers in closeout evidence.

## Claims

“Himsat is the best” is an ambition, not a repository fact.

Benchmark claims must name:

- dataset/corpus and license;
- device/hardware/software versions;
- model and exact hash/version;
- protocol and metric;
- baseline products/versions where comparison is permitted;
- run artifacts and failure cases;
- date and reproducibility instructions.

Negative results remain evidence and must not be hidden.

<!-- graft:start -->
## Graft — repository context layer

Use Graft (https://github.com/trailhq/Graft, `@nanonets/graft`) only as local developer/agent repository-context and navigation tooling. It does not alter Himsat product authority, recording/privacy behavior, platform claims, donor admission, or provenance requirements.

If Graft is unavailable or the local `graft/` graph is absent/stale, run `graft init`, select the active agent(s), then run `graft build`. Before broad source exploration prefer `graft check`, `graft map`, `graft ask "<question>" --source`, `graft skeleton <file>`, `graft callers <symbol>`, and `graft grep "<literal>"`. After material code changes, run `graft build` again.

Treat `graft/` as a local regenerable cache and do not commit it. Graft must index repository code/context only; never place captured audio, transcripts, speaker embeddings, screenshots, credentials, secrets, or other private runtime/user data into its context graph. Keep usage zero-cost and local; do not introduce paid model/API usage, telemetry, or hidden remote fallback.

Graft output is context only, never PASS/VERIFIED evidence, privacy/security proof, platform capability evidence, donor/provenance evidence, benchmark evidence, or completion authority. Continue all SpecGrain, Diffcipline, exact-head, test, Jev where applicable, Alibaba Open Code Review, CI, security, provenance, and post-merge gates. Never fabricate Graft output, execution, reviews, or evidence.
<!-- graft:end -->
