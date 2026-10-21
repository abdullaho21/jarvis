# Local speech decision and evidence

The measurements below document the initial PocketSphinx implementation.
The current installed path uses Vosk + Whisper small; see the
[2026-10-21 memory and background check](../memory-background.md).

Checked on 2026-10-20. The chosen initial backend is PocketSphinx 5.1.1 with its
US English acoustic model, VAD-gated keyword spotting, and a separately armed
command grammar. It runs on CPU. There is no cloud request, general statistical language model,
neural inference runtime, or GPU allocation in this backend. Hands-free mode
keeps the small decoder resident. Push-to-talk can terminate the worker after
each command so its native allocations are returned to the OS.

This choice favors measured footprint over unrestricted dictation quality.
Unknown application names need pronounceable aliases. This is a constrained
English command controller; recognition across accents, rooms, microphones,
and games still needs a representative user trial.

## Alternatives and maintenance

| Backend | Maintenance evidence | License/model evidence | Cost and decision |
| --- | --- | --- | --- |
| PocketSphinx 5.1.1 | PyPI release 2026-06-06; upstream describes limited ongoing development and older recognition technology. | BSD-style code notices; acoustic model has Alpha Cephei redistribution terms; CMUDict has CMU redistribution terms. | Selected. Installed acoustic files total 6,609,581 bytes and dictionary 3,275,645 bytes. The wheel also ships a 27,114,385-byte general language model that this application does not load. |
| Vosk 0.3.45, small-en-us-0.15 | Current PyPI release is dated 2022-12-14; do not infer an inactive repository from Python release age alone. | API Apache-2.0; selected model explicitly Apache-2.0, 40M download. | Measured alternative below. Larger resident cost; a lazy subprocess would free memory between commands but add load latency and require a separate activation detector. |
| whisper.cpp | Current release page lists 1.9.5, 2026-10-06. | MIT implementation; check exact model artifact terms before redistributing a conversion. | Upstream reports tiny: 75 MiB disk / about 273 MB memory, base: 142 MiB / about 388 MB. CPU-only is supported. Not installed or measured here; a candidate for optional active-only recognition, not idle streaming. |
| openWakeWord | README's latest highlighted release is 0.6.0, 2024-02-11; no stronger maintenance claim is made. | Code Apache-2.0; bundled pretrained models CC-BY-NC-SA-4.0, which constrains redistribution and commercial use. | 16 kHz PCM, 80 ms model frames; still needs a command recognizer and runtime. Not installed or measured here. Avoid bundling its models under an assumed permissive project license. |

Primary sources: [PocketSphinx package and release metadata](https://pypi.org/project/pocketsphinx/),
[PocketSphinx README](https://github.com/cmusphinx/pocketsphinx),
[code notices](https://github.com/cmusphinx/pocketsphinx/blob/main/LICENSE),
[acoustic model terms](https://github.com/cmusphinx/pocketsphinx/blob/main/model/en-us/en-us/README),
[CMUDict terms](https://github.com/cmusphinx/cmudict/blob/master/LICENSE),
[Vosk package](https://pypi.org/project/vosk/),
[Vosk API license](https://github.com/alphacep/vosk-api/blob/master/COPYING),
[Vosk models](https://alphacephei.com/vosk/models),
[whisper.cpp README](https://github.com/ggml-org/whisper.cpp),
[whisper.cpp releases](https://github.com/ggml-org/whisper.cpp/releases),
[openWakeWord README and licensing](https://github.com/dscripka/openWakeWord).

No models, dictionaries, audio recordings, or speech fixtures are copied into
this repository. Installing the pinned upstream package brings its bundled
assets and notices. A distributable binary/package must preserve upstream
notices. No license has been selected for this project's own source.

## Actual isolated measurements

Machine: AMD Ryzen 7 7800X3D, 16 logical CPUs, approximately 32 GB RAM,
Linux 7.2.10 CachyOS, Python 3.14.8. Fresh processes per case. Peak RSS includes
the Python interpreter and engine. CPU percentage is process CPU time divided
by elapsed wall time times 100, so 100% means one logical CPU. Full precision
results and fixture checksum are in [speech-benchmark.json](../benchmarks/speech-benchmark.json).

| Case | Observed result | Limits |
| --- | --- | --- |
| PocketSphinx wake standby, silent PCM | 0.0506% of one logical CPU over 15 seconds; 47.48 MiB peak RSS; zero events. | Paced 30 ms frames; excludes microphone capture and IPC. |
| PocketSphinx wake standby, seeded white noise | 0.0558% over 15 seconds; 47.23 MiB peak RSS; zero events. | Repeated low-amplitude synthetic noise frame, not gaming audio or room-noise evaluation. |
| PocketSphinx active command | 20/20 decodes of one clean upstream utterance matched exactly; median 16.59 ms / p95 17.05 ms; 49.65 MiB peak RSS; model/grammar construction 53.95 ms. | Same 2.78625-second recording repeated, plus one second silence. Offline decode time, not measured end-to-end microphone latency or a 20-speaker accuracy test. |
| Vosk small model | 205.27 MiB peak RSS; import/model load 215.40 ms; constrained median 51.67 ms, open vocabulary median 362.58 ms. Both recognized the same clean utterance. | Peak covers both decoder modes in one process. 10 repeats; only one clean speech sample. |

The measurements support choosing PocketSphinx for the initial resident budget.
They do not prove the full service's idle CPU target, the end-of-speech latency
target, or recognition reliability in a game. See the full-system benchmark for
capture/service/UI costs. Human voice, the default wake phrase, echo from
speakers, different accents and microphone-error recovery require local
acceptance testing. A headset helps isolate game output but is not a guarantee.

The keyword path was separately exercised using the fixture's phrase “go
forward” as the test wake phrase. It emitted one wake event at sensitivity 0.5
(`kws_threshold=1e-20`) and none at sensitivity 0 (`1e-10`). This verifies the
activation adapter, not the default wake phrase's real-world false-accept rate.

## Paced full-application integration

[scripts/check_replay.py](../../scripts/check_replay.py) now exercises both
push-to-talk and hands-free activation through the actual capture worker,
PocketSphinx adapter, private daemon socket, confidence gate, deterministic
custom alias parser, dry-run dispatcher, and status feedback. A temporary fake
`pw-record` supplies the explicit upstream fixture; the test never opens a
microphone or executes a desktop action.

The hands-free sequence uses the complete fixture twice with a one-second gap,
sets the surrogate wake phrase to “go forward”, and maps the recognized test
phrase to the safe dry-run volume action. Wake activation was observed before
the second fixture dispatched. The measured dispatch was 0.396 seconds after
the final command frame above RMS 0.01, with 50 ms status observation granularity.
The final energetic frame is selected across the whole fixture before playback,
so an early result cannot redefine the endpoint. This is a waveform-energy
proxy and a synthetic replay, not human speech ground truth or live microphone
latency. See [replay-wake-integration.json](../benchmarks/replay-wake-integration.json).

```sh
.venv/bin/python scripts/check_replay.py /tmp/goforward.raw
.venv/bin/python scripts/check_replay.py /tmp/goforward.raw --wake
```

The replay exposed and now guards against a command becoming permanently
unarmed after an unmatched wake-tail segment. Regression checks also verify
that keyword recognition remains active after an unmatched utterance. No local
TTS smoke corpus was available: espeak and flite executables and libraries are
absent; the installed Qt flite plugin has an unavailable `libflite.so.1`
dependency. No TTS runtime was installed. Shipping phrases and the default
wake phrase still need live user testing across voices and game audio.

## Recognition and bounded processing

`SpeechEngine` uses `Decoder(lm=None, samprate=16000, bestpath=True)`,
`add_keyphrase`, `add_jsgf_string`, and `activate_search`. It feeds 16-bit mono
PCM through `Endpointer(sample_rate=16000, vad_mode=..., frame_length=0.03)`;
`frame_bytes` is read from the object, not guessed. The endpointer's 300 ms
window gates decoding. An utterance is closed with `end_utt` before its final
command result is read. The modern API is documented in the
[PocketSphinx reference](https://pocketsphinx.readthedocs.io/en/latest/pocketsphinx.html).

One decoder holds both searches, avoiding duplicate acoustic models. Its
utterance history is reset after at most ten seconds of continuous detected
speech. The caller enforces a shorter configurable command deadline and drops
its bounded PCM frame after processing. One command arm returns at most one
recognized result; an unmatched speech segment leaves it armed until that
deadline. Keyword searches also restart after unmatched utterances. After a wake
match, the capture worker waits for 210 ms below the configured energy gate
before arming the command grammar; this discards the wake phrase's tail. Pause
between wake and command. Pause/mute cancels the arm and destroys the microphone
worker.

Sensitivity is 0–1. Higher values lower the KWS likelihood-ratio threshold
(logarithmic range 1e-10 to 1e-30) and reduce VAD aggressiveness. This is a tuning
control, not a guarantee of a particular false-accept rate. Use a multiword wake
phrase, pause after activation, speak the command, and adjust using real room
and game audio.

The command's `confidence` is the decoder hypothesis posterior with `bestpath`
enabled; the separate minimum word posterior is diagnostic. Neither score is
calibrated as the probability the user intended a command. Constrained grammars
can assign high confidence to unrelated speech. The initial configurable 0.35
acceptance threshold is a starting point requiring user testing. This fixture
produced about 0.494 hypothesis posterior and 1.0 word posteriors; two
alternative silence paths affect the global score. Without `bestpath`, the
upstream API warns that `get_prob()` can simply return 1.0. Fixed-base logmath
rounding can produce 1.0001, which the adapter bounds to 1.0. Wake events have
`confidence=None`: KWS already uses its own likelihood-ratio acceptance gate.

Grammar strings are constructed only from tokens present in the pronunciation
dictionary. Unsupported phrases are reported to Settings instead of inventing
pronunciations. In the inspected desktop registry, 85 installed apps produced
378 candidate phrases, of which 135 were speakable and 243 were excluded;
several aliases can refer to the same app. “Stremio” is absent from this dictionary;
an alias made from known words, such as “stream video”, resolves that limitation.
Names still appear in the application list; create a pronounceable alias to enable voice launching.

## Reproduce

Install the normal pinned voice dependency using the README's isolated setup.
Download the upstream non-private test fixture into `/tmp`, and check its hash:

```sh
curl --fail --location https://raw.githubusercontent.com/cmusphinx/pocketsphinx/main/test/data/goforward.raw -o /tmp/goforward.raw
sha256sum /tmp/goforward.raw
# f15c60ec54059d8b66e410d0064945a0b0a04ea56e1ddca1958e493c0cf70e71
.venv/bin/python scripts/benchmark_speech.py --case silence --seconds 15
.venv/bin/python scripts/benchmark_speech.py --case white_noise --seconds 15
.venv/bin/python scripts/benchmark_speech.py --case active --fixture /tmp/goforward.raw
PYTHONPATH=src MURMUR_SPEECH_TEST_RAW=/tmp/goforward.raw .venv/bin/python -m unittest discover -s tests -p test_speech.py -v
```

Optional comparison only (Vosk is not a production dependency):

```sh
.venv/bin/python -m pip install vosk==0.3.45
curl --fail --location https://alphacephei.com/vosk/models/vosk-model-small-en-us-0.15.zip -o /tmp/vosk-small.zip
unzip /tmp/vosk-small.zip -d /tmp
.venv/bin/python scripts/benchmark_speech.py --case vosk --fixture /tmp/goforward.raw --vosk-model /tmp/vosk-model-small-en-us-0.15 --repeats 10
```

Source and fixture provenance: [upstream audio test directory](https://github.com/cmusphinx/pocketsphinx/tree/main/test/data).
The script never opens a microphone or automatically downloads a model. Default
unit tests skip fixture-dependent checks unless its explicit path is provided.
Twelve speech-specific checks passed with the fixture enabled on this machine.
