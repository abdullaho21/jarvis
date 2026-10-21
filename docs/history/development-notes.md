# Historical development notes

Archived on 2026-10-21. These notes describe earlier versions and are not the current setup guide. See the root README for Jarvis.

# Murmur

A small local voice controller for Hyprland, with a separate native GTK 4
settings window. Say **“hey Jarvis, open Firefox”**, **“hey Jarvis, go to
workspace three”**, or **“hey Jarvis, start recording”**, in one breath or with
a pause after “hey Jarvis”.

**Hands-free wake activation is the default, after first-run microphone consent.**
Push-to-talk is an optional listen-once alternative. Mute closes the voice input
and releases its model. Closing settings releases the UI; the service continues.

![Native GTK settings connected to the service, before microphone consent](../ui-preview.png)

This is a working initial implementation with measured resource use and known
gaps, not a finished recognition-quality claim. The default English recognizer
needs aliases for names outside its pronunciation dictionary. Murmur is a working
name, and no source license has been chosen yet (see LICENSE).
Murmur never edits your Hyprland configuration.

An October 2026 code review found and fixed several problems; see
[Review fixes](#review-fixes-october-2026) at the end of this file.

The current Vosk + Whisper small recognizer also has a measured RAM reduction
and a fix for commands decoded after the capture deadline. See
[memory measurements and background verification](../memory-background.md).

## What works

- CPU-only local wake-word detection, bounded command recognition, confidence
  gating, cancellation and duplicate suppression. No cloud, API key, LLM, GPU
  voice inference, or continuous large recognition model.
- Installed XDG desktop application discovery, exact aliases, ambiguous-name
  errors, desktop-native launching, and focusing an existing matching window.
- Workspaces one–ten, silent window moves, idempotent fullscreen, and volume.
- Headless start/stop of a preconfigured `wf-recorder` source, private staging,
  graceful stop, video validation, saved-file feedback and crash retention.
- Six GTK pages: overview, searchable commands, app aliases, recorder,
  microphone/activation, and privacy/startup. Optional recording tones and an
  event-driven Waybar status surface.

No destructive actions or arbitrary spoken shell commands are implemented.
Adding an alias cannot add an executable or shell script.

## One-time setup

Tested on CachyOS/Arch, Python 3.14.8, GTK 4.24.1, Hyprland 0.56.2,
PipeWire 1.6.9 and wf-recorder 0.6.0. Python 3.11+ is required. Other distributions
and Hyprland versions need the integration checks below; they are not claimed
tested. Use your distribution's GTK/PyGObject packages rather than building a
second GTK with pip.

On CachyOS/Arch, review these system packages before installing them:

```sh
sudo pacman -S --needed python python-gobject gtk4 python-setuptools \
  pipewire pipewire-pulse wireplumber wf-recorder ffmpeg \
  base-devel cmake ninja
```

PipeWire and WirePlumber must already be running in your desktop session.
Then, inside this checkout, create a local environment and install Murmur:

```sh
python3 -m venv --system-site-packages .venv
.venv/bin/python -m pip install -e '.[voice]'
./run-murmur install-model        # one-time download of the voice models (about 680 MB)
.venv/bin/python scripts/install_user.py --dry-run --service
.venv/bin/python scripts/install_user.py --service
```

The optional systemd launch path needs systemd 254+; manual foreground operation
does not. The tested machine uses systemd 262.

The installer creates only your **Murmur desktop launcher** and an optional,
initially disabled **user service**, then reloads the user service definitions.
It prints the exact destination paths. Existing differing files require
`--replace`; review them first. It never edits Hyprland. Keep the checkout and
venv at their installed paths, or reinstall the launcher/service after moving.

Open **Murmur** from your application launcher. Choose **Start service**, then
review **Privacy & startup** to grant microphone consent. The default wake mode
keeps the input and small decoder available while unmuted. Say the wake phrase,
pause for activation, then one command. The screen shows listening/processing/
paused/error states. **Mute microphone** closes voice capture immediately.

Install with `-e` (editable) as shown, so the checkout's `src/` is what runs and
your edits take effect without reinstalling. From the checkout, `./run-murmur`
also opens settings once installation is done; `./run-murmur daemon` runs the
service in the foreground. `run-murmur` works through a symlink (for example
`ln -s "$PWD/run-murmur" ~/.local/bin/murmur`) and prints the exact rebuild
command if the venv is missing or broken.

**After a system Python upgrade** (CachyOS/Arch move to a new 3.x), the venv
still points at `/usr/bin/python3` but its packages live under the old version,
so the launcher, user service and `run-murmur` all stop working. Rebuild it:

```sh
rm -rf .venv && python3 -m venv --system-site-packages .venv
.venv/bin/python -m pip install -e '.[voice]'
```

`./run-murmur doctor` reports `"venv_matches_python": false` in that state. Normal desktop commands,
setup and reference browsing all work in the UI without a terminal.

Murmur uses the **Vosk** recognizer when its model is installed
(`./run-murmur install-model`, or **Download better voice model** on the Voice
page). The download is checked against a pinned SHA-256 and stored in
`~/.local/share/murmur/models/`. Without it, Murmur falls back to the basic
PocketSphinx recognizer bundled with its Python package, which handles accents
poorly. No model is bundled in this repository. See
[dependency/model licensing](../dependencies.md).

## Giving an app a voice name

Pick any app from your launcher and choose what you'll say to open it.

- **In the window:** Applications → search for the app → **Add voice name** →
  type the name (e.g. `streamio`) → press Enter. The app row then lists its
  voice names.
- **In a terminal:**

  ```sh
  "/absolute/path/to/checkout/run-murmur" alias streamio stremio
  ```

  The second word is the app as it appears in your launcher; part of the name
  (`strem`) or its desktop ID (`com.stremio.Stremio.desktop`) also works. Run
  `run-murmur alias` alone to list voice names, or add `--remove` to delete
  one.

Then say “Jarvis, open streamio”. The name doesn't have to be an English word.
If the model doesn't know it, Murmur splits it into two words it does know
(“streamio” → “stream io”, “palworld” → “pal world”), and closest-match covers
the rest. A voice name and the app's real name count as the same app, so they
never block each other.

## Discovering phrases and aliases

Open **Commands** to search examples. These are accepted phrases:

| Intent | Example |
| --- | --- |
| Launch or reuse an installed application | “open Dolphin”, “launch Firefox”, “start Firefox” |
| Focus an existing app | “focus Firefox”, “switch to Firefox” |
| Workspace | “go to workspace three” |
| Move active window, preserve current workspace | “move this window to workspace two” |
| Fullscreen | “make this window fullscreen”, “exit fullscreen” |
| Audio | “turn the volume down”, “turn the volume up”, “mute volume”, “turn the sound back on” |
| Recording | “start recording”, “stop recording” |
| Open recorder setup without capturing | “open the video recorder” |
| Cancel / close voice input | “cancel”, “stop listening” |

Any command may start with “please”, “can you”, “could you” or “would you”
(optionally followed by “please”), e.g. “could you open Firefox”. Plain commands
are the most reliable: longer polite forms tend to get lower scores, so they are
more often rejected by the score threshold. Typed commands (`murmur execute`)
also accept “open up …” and a trailing “please” or “for me”; the recognizer does
not offer those because they measurably lowered recognition scores.

The recognizer knows ordinary English words, not every product name. With Vosk,
**closest-match guessing** covers most unusual names: “open Stremio” is heard
as something like “open stream io”, and the closest command is chosen, but only
when it is clearly closer than any other command. The status then shows what
was heard (“Heard ‘open stream io’ → open stremio”). You can switch this off on
the Voice page. Numbers in app names can be spoken (“open Tekken eight”). An
alias still helps for names that are nothing like English words; for example,
map “web browser” to Firefox. If the same app is installed twice (e.g. native
and Flatpak Firefox), the native one opens; add an alias to choose the other.

Command shortcuts map a whole phrase, such as **“capture that”**, to an existing
fixed command, such as **“start recording”**. They cannot invoke arbitrary shell
commands. The configurable score is an **uncalibrated grammar posterior**, not a
probability that speech was understood correctly. Noise can still cause false
activations or matches; test with your voice, headset and game.

## Recording setup

Open **Recorder**, select the exact monitor, output folder, frame rate, encoder,
quality and optional audio source. Audio is off initially. Check capture
permission outside a game, test playback and a second unattended start, then
acknowledge setup and save. Opening the recorder never starts capture.

The default CPU preset is portable but measured about **196% of one logical CPU**
for a short native 1440p30 desktop capture on this machine. It can affect gaming;
do not equate low voice overhead with low video-encoding cost. Review the
VA-API preset (6.47% child CPU in a short capture here) and its verification limits in [recording](../recording.md)
and [performance](../performance.md) before gaming.

wf-recorder accepts one audio source. The UI discovers explicit microphone and
output-monitor sources. Recording both requires a separately configured combined
PipeWire source and acknowledgment. Murmur does not silently alter audio routing.
**Muting voice input does not mute an existing recording's microphone track**;
stop recording to end that capture. A visible recording indicator is available
in settings and through the optional Waybar module.

Direct Wayland capture is subject to compositor permission policy. A portal may
show a picker or require renewed consent; this application does not bypass it.
The verified setup uses direct wf-recorder capture with Hyprland enforcement
currently disabled. Reboot persistence and fullscreen-game behavior still need
interactive verification. See [recording setup and recovery](../recording.md).

## Startup and status bar

After installing the optional unit, enable **Start at login** in Privacy &
startup. Hyprland's graphical session must export its environment to the user
service manager. If your session does not do so, run this once from that session
before starting the user service:

```sh
systemctl --user import-environment WAYLAND_DISPLAY HYPRLAND_INSTANCE_SIGNATURE XDG_CURRENT_DESKTOP XDG_RUNTIME_DIR
systemctl --user start murmur.service
```

This does not edit your compositor configuration. The service uses the
`graphical-session.target`; verify that your session starts/stops that target.
The UI's Start service button is also available if your session does not manage
it. Stop a manually started service before starting the installed unit; a lock
prevents duplicates. Startup preference saves enablement, not a silent restart
of an already-running manually launched process.

Review and manually merge [the optional Waybar example](../../examples/waybar.jsonc).
It streams changes rather than polling. Click opens settings, right-click mutes
voice. [Shortcut examples](../../examples/hyprland.conf) are optional and never installed
automatically; match syntax to your Hyprland version. Listen-once shortcuts do
not require the settings window and do not focus it.

## Performance and verification

Measured on the Ryzen 7 7800X3D test machine, 20 October 2026. CPU percentage is
relative to **one** logical CPU; RSS sums shared pages conservatively. Short
samples and workload limits are documented in [performance](../performance.md).

| Cost | Observation |
| --- | --- |
| Hands-free waiting, live input, 30 seconds | 0.17% CPU; 83.80 MiB peak service+voice+capture RSS |
| Muted service, 15 seconds | No CPU ticks recorded; 26.95 MiB RSS |
| Push-to-talk standby, 15 seconds | No CPU ticks recorded; 27.10 MiB RSS; input closed |
| Native Wayland settings+PTT service, 15 seconds | 0.13% CPU; 164.04 MiB combined RSS |
| Paced hands-free fixture replay | Successful wake→command→dry-run action; 0.40 s after documented energy endpoint proxy |

The earlier 30 second sample measured 0.56% CPU and 85.74 MiB; both samples
meet the initial waiting CPU/memory targets. Short-sample variation is expected.
They do not prove long-run false-accept rates, human recognition accuracy,
live end-of-speech latency, or game frame-time impact. No GPU inference is used
by the CPU-only voice path. The UI and optional video media engine have separate
costs. [Speech research](../speech-research.md) compares measured PocketSphinx
and Vosk costs and the maintenance/accuracy tradeoff.

Run unit and integration checks after setup:

```sh
.venv/bin/python -m unittest discover -s tests -v
./run-murmur doctor
.venv/bin/python scripts/check_desktop.py
```

The last command launches only a harmless temporary desktop test entry. Real
screen/microphone tests are explicit opt-ins. Full commands, tested boundaries,
failure cases and remaining checks are in [verification](../verification.md).
Benchmark scripts never download a model automatically and keep production
configuration separate in temporary directories.

The suite has **91 tests**. Five use the upstream PocketSphinx recording
`goforward.raw` and are skipped unless `MURMUR_SPEECH_TEST_RAW` points to it.
The GitHub Actions workflow runs the offline tests; it has not run on GitHub yet.

## Project map

- [Architecture and activation policy](../architecture.md)
- [Native UI, accessibility and verification](../ui.md)
- [Recording, permissions, presets and partial-file recovery](../recording.md)
- [Measured performance and reproduction](../performance.md)
- [Troubleshooting](../troubleshooting.md)
- [Example configuration](../../examples/config.json), stored in `$XDG_CONFIG_HOME/murmur/config.json`
- [Dependency/model terms and pending release decisions](../dependencies.md)

Settings are validated and written atomically as a private 0600 JSON file. Audio
and transcripts are discarded after processing; no history, telemetry or cloud
upload is implemented. Recording files are intentional user output and use 0600
permissions. See the architecture for separate microphone and recorder lifetimes.

## Review fixes (October 2026)

A review of the code found the problems below. Each one is fixed, and each fix
has a regression test in `tests/test_review_fixes.py`. The pre-fix source is
kept in `_to_delete/pre-review-fixes-snapshot.tar.gz` (git-ignored; delete it
when you no longer need it).

### High severity

1. **Documented phrases could not be spoken.** The recognizer's grammar was built
   separately from the parser and only knew “open/launch/focus *app*”. Phrases the
   Commands page listed, such as “please open Firefox” and “could you launch
   Firefox”, plus “start/switch to *app*” and “go to the workspace *n*”, were
   accepted by the parser but could never come out of the recognizer. The tests
   only checked the parser, so they passed anyway.
   *Fix:* `Parser.grammar()` now builds the recognizer grammar from the parser's
   own tables (`commands.py`), and `GrammarMatcher` checks recognizer output
   against that same grammar. App names are a separate grammar rule, so the
   grammar grows by one entry per app instead of three. New tests check that
   every Commands-page example is speakable, and that every sentence the
   grammar allows parses. Synthetic-speech checks showed that how the grammar
   is written changes PocketSphinx's scores. With an optional `[please]` prefix
   or optional trailing words, plain commands scored lower (0.49 → 0.33 on the
   upstream recording), and trailing words made the decoder drop commands. The
   grammar is therefore written so that plain-command scores stay where they
   were (a test enforces this).
2. **“Open *game*” focused the Steam window instead of launching the game.** To
   find an already-running app, Murmur treated the first word of a desktop
   entry's `Exec=` line as a window class. Every Steam game's `Exec` is
   `steam steam://rungameid/N`, and the Steam client's window class is `steam`.
   Browser web apps (`google-chrome --app-id=…`), `flatpak run …` and `env …`
   launchers had the same problem.
   *Fix:* known launcher programs are ignored (`LAUNCHER_WRAPPERS` in
   `apps.py`). Browser web apps match by their own class, and Steam games
   match their real window class `steam_app_N`.
3. **The screen recorder kept recording if the service died.** `wf-recorder`
   ran in its own session with no link to the service. If the service crashed
   or was killed while started by hand (`./run-murmur daemon` or the UI's Start
   service button), the capture kept going until the disk filled. A restarted
   service didn't know it existed.
   *Fix:* the recorder now starts through `setpriv --pdeathsig INT`, so it
   receives SIGINT, finalizes its file and exits when the service dies. It is
   started from one long-lived thread, because Linux sends that signal when the
   *thread* that started the child exits. On startup the service reports a
   still-running recorder from an earlier session, with the PID and a
   `kill -INT` command, and refuses to start a second one. It also lists any
   leftover `.Murmur_*.part.mkv` files.
4. **A settings file or a big app library could stop the service from
   starting.** A custom command with a non-built-in target passed validation,
   then made the service exit at startup. More than 4,096 grammar phrases
   (about 650 desktop entries, which large Steam/Lutris libraries reach) also
   made it exit, and systemd then restarted it every 3 seconds.
   *Fix:* custom commands are checked when settings are validated. Apps beyond
   2,048 are reported as unavailable instead of being fatal. If voice control
   cannot start, the service still runs and shows the error instead of exiting.

### Medium severity

- **Waybar module disappeared while the service was off,** and was re-launched
  every 5 seconds. `murmur waybar` now shows an “off” state and reconnects on
  its own, waiting 2 s and then longer between attempts, up to 30 s.
- **Background tasks could be lost silently.** asyncio only weakly references
  tasks it isn't awaiting, so an automatic voice restart could vanish or fail
  without a visible status. They are now tracked (`Service.background`) and
  failures appear in the status.
- **“Mute volume” toggled,** so saying it twice unmuted. It now always mutes,
  and “turn the sound back on” unmutes (“unmute” is not in the recognizer's
  dictionary).
- **Slow work blocked the service, and the app list went stale.** Saving
  settings re-scanned every desktop file on the event loop. That now runs in a
  worker thread. Opening or reconnecting the settings window re-scans apps,
  which picks up newly installed ones and restarts the listener if the list
  changed.
- **`murmur record stop` could report a timeout even though the file saved.**
  Its 12 s client timeout was shorter than finalizing plus validating the
  recording. Slow operations now have longer timeouts (`client.TIMEOUTS`).
- **Unused Vosk code and dependency.** `models.py`, `recognition.py` and
  `active_speech.py` were never imported, and the `recognition` settings
  section was ignored. The unused modules and
  `data/org.murmur.Controller.desktop` were moved to `_to_delete/`. (Vosk then
  became the main recognizer; see the next section. `models.py`, the verified
  model installer, is back in use.)
- **Settings validation gaps.** Misspelled setting names (e.g.
  `comand_timeout`) used to be saved and silently ignored. They are now
  rejected when saving and dropped with a warning when loading. The recorder
  and settings now agree on 1–60 fps. `vaapi_device` is a validated setting,
  files are read and written as UTF-8, and saves are flushed to disk with the
  folder.
- **The UI's Start service button ran an unmanaged service with no log.** It
  now starts the installed user service when there is one. Otherwise it writes
  a private log at `~/.local/state/murmur/daemon.log` and cleans up the process
  when it exits.
- **The venv broke on a Python upgrade with no hint.** `run-murmur` and
  `murmur doctor` now detect it and print the rebuild command (see One-time
  setup).

### Smaller fixes

- `run-murmur` works when symlinked and gives a clear message instead of
  `not found` when the venv is missing.
- The README now uses an editable install (`-e`). Without it, `run-murmur`
  silently ran an old copy instead of your edited `src/`.
- `doctor` now checks every program Murmur runs (`pw-record`, `pw-play`,
  `pw-dump`, `pactl`, `setpriv`, `systemd-run` were missing).
- Desktop entries marked `OnlyShowIn`/`NotShowIn` for other desktops are
  ignored, which removes false “ambiguous app” errors from GNOME/KDE-only
  entries.
- The app picker for aliases is searchable, and searching the app list no
  longer resets its selection.
- Removed code that only looked like privacy protection (clearing a dict and
  hashing the last command) and the hard-coded Stremio hint in the UI. Also
  removed unreachable phrase entries and fixed linter findings.
- `.aws/`, `.codex/`, `.agents/` and `_to_delete/` are git-ignored, so local
  credentials or tool state can't be committed by accident.

Not changed: localized desktop-entry names, pinning GitHub Actions to commit
SHAs, and trimming the long docs. Live microphone, recording and Hyprland
behavior after these fixes still needs testing on the real desktop. The
automated checks used fake processes, synthetic speech and the upstream
recording.

## Voice recognition upgrade (October 2026)

After the review, real use showed that recognition wasn't good enough. The
wake phrase often needed four tries, and “open Firefox” failed with “ambiguous
application”. These changes fix that.

1. **New recognizer: Vosk.** PocketSphinx is old and handles accents poorly.
   Murmur now uses the Vosk (Kaldi) small English model when it is installed
   (`murmur install-model`, a one-time 40 MB download with a pinned SHA-256).
   It stays fully offline and CPU-only, and PocketSphinx remains the fallback
   when no model is installed (`recognition.backend` in the settings can
   force either). Cost: about 185 MB RAM while the microphone is on (was about
   50 MB). Silence costs almost no CPU, because quiet audio is not decoded.
   About 2% of one CPU core with constant loud input while waiting for the
   wake word, and about 7% during a command.
2. **New wake word: “hey Jarvis”.** “Jarvis” is distinctive, and only that part
   must be heard clearly: “hi Jarvis”, “okay Jarvis”, “Jarvis”, or a misheard
   greeting all work. It must come at the start of what you say, so “Jarvis” in
   the middle of a sentence doesn't wake it. A false wake only opens a short
   command window; nothing runs without a command. You can change the phrase on
   the Voice page.
3. **One breath is fine.** “hey Jarvis open Firefox” works without pausing
   after the wake words. A pause still works too.
4. **Closest-match guessing** (your “closest word” idea). Alongside the strict
   command grammar, a second Vosk pass writes down freely what you said, using
   the same model, so no extra memory. If the grammar can't match it, the
   closest command is chosen, but only when it scores at least 0.82 similarity
   *and* beats every *different* command by 0.06. The two passes also check
   each other: when they point to different commands, nothing runs unless the
   free transcript is a near-exact match. Doing nothing is safer than doing the
   wrong thing. The status always shows what was heard.
5. **The soft start of “hey” is no longer cut off.** Audio below the energy
   gate used to be replaced with silence, which could clip quiet word starts.
   Vosk now gets the 300 ms of audio before speech starts, and the default
   gate is lower (0.005; it was 0.01).
6. **“Open Firefox” was ambiguous** because Firefox was installed twice (the
   native `firefox.desktop` and the Flatpak `org.mozilla.firefox.desktop`).
   The old code required an alias. Now, when entries share a name *and* the
   same window class, they are treated as one app and the native one opens.
   The status names the other copy; add an alias to choose it. Different apps
   that merely share a name still ask for an alias.
7. **App names.** Numbers can be spoken (“Tekken eight” for “TEKKEN 8”), and
   names containing dots are no longer split
   (“F.E.A.R. 2 Project Origin” used to be indexed as “2 project origin”).
8. **Window actions failed with a Lua Hyprland config.** Since Hyprland 0.55
   the config can be written in Lua, and then `hyprctl dispatch` expects Lua
   dispatchers (`hl.dsp.focus({ window = "address:0x…" })`), not the old
   `focuswindow address:0x…`. That is why “open Firefox” with Firefox already
   running said “Hyprland rejected the action: … expected near 'address'”.
   Murmur now sends the Lua form when `~/.config/hypr/hyprland.lua` exists. It
   falls back to the other form if the first one is rejected, and remembers
   which one works. This covers focusing a window, switching workspace, moving
   a window to a workspace and fullscreen. Shortcut examples for a Lua config
   are in `examples/hyprland.lua`.

**How it was measured.** There are no recordings of your voice, so these are
synthetic voices (espeak-ng: American, British, Scottish, Caribbean, West
Midlands, Arabic-voiced English, plus quiet and noisy copies). They are a proxy
for accents, not a substitute for testing with your voice, but the gap is
large:

| Synthetic test set | PocketSphinx (old) | Vosk + closest-match (new) |
| --- | --- | --- |
| Wake phrase detected | 0 of 24 | 23 of 24 |
| Command correct | 44 of 130 | 92 of 130 |
| Wrong command run | 13 | 4 |
| Wake + command in one breath | not supported | 16 of 18 |

Most remaining misses are made-up names like “Stremio” and “Tekken”, whose
synthetic pronunciation is unrecognizable. Tests: `tests/test_voice_upgrade.py`
(16 tests; the real-model ones run when `MURMUR_VOSK_MODEL` points to the
model).

## Accurate recognition upgrade (October 2026)

Vosk alone still missed commands too often for a non-US accent, and a voice
in the background made it worse. This round adds a second, much stronger
model, still offline and CPU-only.

1. **Whisper small for commands.** Vosk keeps doing the cheap, always-on part:
   hearing “Jarvis” and noticing when you stop talking. The few seconds of
   your command are then written down by OpenAI's Whisper small (English),
   which was trained on far more accents. The closest-match step picks the
   command from that transcript; Vosk's own result is kept as a cross-check.
   The model runs through sherpa-onnx (int8, CPU) and is downloaded once
   (about 636 MB, SHA-256 checked) by `run-murmur install-model` or the Voice
   page button.
   *Cost:* about 800 MB of RAM while the microphone is on, and about 0.5 s per
   command on two slow cloud cores (faster on a desktop CPU; it uses up to
   half your cores, at most 8). Turn off **Accurate model for commands** on the
   Voice page to go back to Vosk only.
2. **Background voices and sound.**
   - *Adaptive threshold* (**Ignore background sound**, on by default). The
     speech threshold follows the room's steady background level (TV, game,
     music, fan) and sits about 10 dB above it, so only a voice close to the
     mic counts. The microphone test now shows the current threshold.
   - *End of command.* Murmur decides when you stopped talking, after 0.6 s of
     quiet, instead of waiting for Vosk. A TV still talking far away could
     keep Vosk waiting.
   - *Deadline.* When the 8-second window runs out, what you said so far is
     still used instead of thrown away.
   - *Extra words ignored.* If the transcript also caught the TV after your
     command (“start recording tomorrow will be cloudy”), the command at the
     start is still found. Up to two stray words before it are tolerated too.
3. **Digits.** “Go to workspace 3” as written by Whisper counts as “three”.
4. **Noise suppression** (GTCRN, 0.5 MB, optional). It can clean steady noise
   out of the command audio. It's **off by default**: on the synthetic tests it
   slightly lowered accuracy, and no noise suppressor removes *other voices*,
   which is what you described. Try it on the Voice page if your room has
   fan or hum noise.
5. **Mic test shows the threshold**, so you can see whether your voice clears it.

**Measured on the synthetic test set** (161 commands in 7 voices including
Arabic-accented English, with quiet, noisy and *background-voice* copies):

| | Vosk only | Vosk + Whisper small |
| --- | --- | --- |
| Commands right, no background voice | 109 of 130 | 119 of 130 |
| Commands right, another voice talking at the same time | 0 of 31 | 5 of 31 |
| Wrong command run | 6 | 4 |

Other models tested: Parakeet TDT 0.6B was faster (about 0.26 s) but less
accurate here; Whisper base and Moonshine were worse. Someone talking *at the
same time* as you is still hard for every model; pointing the cardioid
SoloCast at your mouth (it rejects sound from the back) helps more than any
software setting.
