# Jarvis memory plan: progress report

**Status: 27 October 2026.** Snapshot of the memory-plan campaign and saved
measurements. This report summarizes [the plan](memory-plan.md); the
[technical handoff](memory-plan-handoff.md) contains reproduction details.

## How far along are we?

**We are past initial prototyping and into architecture selection.** The
measurement tools, storage foundations and candidate recognition components
are built, with substantial isolated Linux evidence. The final combination of
components is still undecided because candidates have recognition, latency or
memory failures.

All **six planned work areas have implementation and measurement work**.
**None has cleared all of its acceptance requirements**, and an integrated
solution has not passed the whole-application or Windows release gates.
These counts describe coverage and acceptance, not the share of effort spent.
A single completion percentage would be misleading: resolving the remaining
candidate failures can change the integration work required.

| Delivery milestone | Position |
| --- | --- |
| Measurement and test infrastructure | Implemented; full workload/corpus evidence incomplete |
| Component prototypes and isolated experiments | Built and measured across all six areas |
| Final architecture and candidate selection | **Current stage; unresolved failures** |
| Integration of the selected solution | Pending selection and required recognition evidence |
| Whole-application, Windows and long-duration acceptance | Not complete |
| Release sign-off | Not granted |

## Progress against the six steps

| Step | Work completed | What keeps it open |
| --- | --- | --- |
| **1. Measure memory and recognition** | Linux/Windows accounting code; application sampler; recognition and soak runners; isolated Linux baseline | Full real-audio corpus, real combined workloads, Windows hardware results, CPU/latency and soak evidence |
| **2. Fix missed wakes** | Rust/Python quiet-boundary reset; repeated background → quiet → wake fixture recovery | Broader recognition and Windows acceptance |
| **3. Move replay buffering to disk** | Opt-in disk mode, private Rust segment store, cleanup/save handling, Windows RAM cap and platform adapters; synthetic savings measured | Strict Linux recorder byte bounds and real capture/storage acceptance on both platforms |
| **4. Explain retained memory** | Vosk history attribution; disabled finalization trial; isolated worker teardown, reader ownership, allocation/cache and pressure controls | Full live-workload explanation where needed, broader pressure and long-duration growth acceptance; experimental policies remain unadopted |
| **5. Reduce second-pass recognition work** | Disabled Vosk agreement fast path avoids Whisper on exact fixture commands and lowers memory | Forced cold-fallback latency regresses; full acoustic acceptance is incomplete |
| **6. Trial smaller models and wake detection** | Rust tiny/base/small Whisper profiles; configurable keyword detector; buffered on-demand command worker; continuous recovery controls | Wake timing/recognition tradeoff; validate the chosen smaller recognizer; select and integrate the final architecture |

## What the measurements establish

The targets cover **all Jarvis processes together**, including capture, replay,
controller and settings when open. Worker-only results cannot pass them.

| Requirement or experiment | Current evidence | Meaning |
| --- | --- | --- |
| Waiting, replay off: **≤150 MiB** | Isolated Linux baseline: **176.84 MiB RSS** | That baseline exceeds the target; final application acceptance is open |
| Waiting, replay on: **≤300 MiB** | Synthetic replay-buffer layer fell from about **266 to 27.5 MiB RSS** | Promising component saving; real capture and the rest of Jarvis are excluded |
| Recognizing commands: **≤400 MiB total** | Reduced handoff setup reaches about **336 MiB with experimental tiny.en** and **622 MiB with small.en** | Small.en already fails at reduced scope; tiny.en still needs whole-app and acoustic acceptance |
| Reliable, timely wake detection | Expanded grid: original quiet gate **288/288** wakes; aligned candidate **287/288**; saved ungated **253/288** | Alignment improves paired timing but loses a wake caught by the old gate, so it is not accepted |
| Standby CPU: **≤0.1% of one core** | Aligned detector silence runs: **0.0148–0.0168%** | Passes this isolated CPU check; recognition remains failed |
| Stable long-run memory | Short cycle/cache/pressure controls completed; three full 24-hour runs not completed | No full-duration plateau or release acceptance |

The expanded wake grid reuses three public recordings with different gains and
leading quiet. It adds timing coverage, not new speakers or a representative
false-alarm rate. There are still no positive recordings of the actual custom
Jarvis wake names in this campaign.

## What remains, in order

1. **Complete the recognition corpus.** Include actual wake names, speakers
   and accents, command confusables, background speech, music and game audio.
   Retain zero false actions as a hard gate.
2. **Resolve candidate failures and choose the architecture.** Find a wake
   path that satisfies recognition, input timing, standby CPU and memory
   together. Validate the chosen command recognizer. The fast path has a cold
   fallback delay; base.en has unresolved reference discrepancies if it is
   retained as a candidate. Tiny.en's fixture results need broader validation.
3. **Finish replay bounds.** Linux disk supervision can overshoot between
   polls. Its current minimum disk budget is 512 MiB because of recorder file
   granularity. Windows has a RAM cap; Linux GSR RAM mode has no byte cap, and
   the earlier recorder-RSS stop was removed. These limits need explicit
   acceptance rather than being counted as a completed strict quota.
4. **Integrate the selected components and settle the Windows runtime path.**
   Verify wake/command buffering, cancellation, recovery, denoising and capture
   together, including GTCRN interaction.
5. **Pass combined application and platform gates.** Measure replay off/on,
   settings open and command peaks, plus CPU/latency and pressure on an 8 GB
   Windows machine or VM and the corresponding Linux workloads.
6. **Run three sequential 24-hour soaks.** Apply the stated warm-up, growth
   tolerance and recognition checks to the integrated candidate. Investigate
   failures and repeat affected acceptance runs.

Steps 1–3 have independent work that can proceed before integration. The final
soak campaign depends on candidate readiness and suitable platform/workload
coverage.

## How much time is left?

**Substantial engineering, integration and acceptance work remains.** There is
no defensible overall ETA until the candidate failures and missing acoustic/
platform evidence are resolved. The final soak campaign alone requires
**at least 72 hours elapsed**, after preparation and candidate readiness;
investigation and reruns add time.

Production Vosk/small.en remains the default. The fast-path and finalization
experiments remain disabled, and the separate KWS probes are not integrated.
Their measured savings are candidate results, not delivered application-wide
savings.

## Supporting reports

- [Measurement coverage, baseline and replay limits](memory-measurement.md)
- [Vosk fast-path savings and cold-fallback failure](vosk-fast-path.md)
- [Smaller Whisper profiles and reference differences](whisper-model-experiments.md)
- [Continuous command handling and reduced worker peaks](kws-continuous-diagnostics.md)
- [Model-aligned wake timing and retained recognition failure](kws-alignment.md)
- [Reader/cache attribution and pressure regression checks](kws-reader-caches.md)
- [Latest isolated KWS code validation: 18 Rust tests per build variant and 10 Python tests](benchmarks/kws-alignment-validation-2026-10-27.json)

Passing code tests verifies the tested implementation. The failed recognition
and memory measurements remain failures independently of those test results.
