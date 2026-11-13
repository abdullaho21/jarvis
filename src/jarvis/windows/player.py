"""Watch a clip inside Jarvis, to find where to cut it.

Windows' own player often can't open what Jarvis records (AV1 in Matroska needs
an extension from the Store), so the Clips page plays clips itself: the bundled
FFmpeg decodes the picture (on the graphics card: this FFmpeg has no software
AV1 decoder) and the sound, Tk shows the frames and PyAudio plays the sound.

It plays the edited timeline: a removed section is skipped and a joined clip
follows, as the exported copy will play. Nothing runs while it is paused.
"""
from __future__ import annotations

import queue
import subprocess
import threading
import time
import tkinter as tk
from tkinter import ttk

PREVIEW_WIDTH, PREVIEW_HEIGHT = 640, 360   # the picture's box; the clip is fitted inside
PREVIEW_FPS = 30
AUDIO_RATE, AUDIO_CHUNK = 48000, 4096   # 16-bit stereo; a chunk is about 21 ms
TICK_MS = 10
FRAME_QUEUE = 8   # decoded frames held ahead; the decoder waits beyond this


def preview_size(video, box=(PREVIEW_WIDTH, PREVIEW_HEIGHT)):
    """The clip's picture fitted in ``box``, in even sizes (as the scaler needs)."""
    try:
        width, height = int(video.get("width") or 0), int(video.get("height") or 0)
    except (TypeError, ValueError):
        width = height = 0
    if width <= 0 or height <= 0:
        return box
    scale = min(box[0] / width, box[1] / height)
    return max(2, round(width * scale / 2) * 2), max(2, round(height * scale / 2) * 2)


def preview_tracks(audio, selection):
    """The sound tracks heard while watching: those kept for export. "All sound"
    already holds the others, so when it is kept it plays alone."""
    from .encoding import MIXED_TITLE

    kept = [t for t in selection if t < len(audio)]
    if 0 in kept and audio and audio[0].get("name") == MIXED_TITLE:
        return [0]
    return kept


def _decoder(path, at):
    # The graphics card decodes (FFmpeg falls back to software for other formats).
    return ["ffmpeg", "-hide_banner", "-v", "error", "-nostdin", "-hwaccel", "d3d11va",
            "-ss", f"{max(0.0, at):.3f}", "-i", str(path)]


def video_command(path, at, length, size, fps=PREVIEW_FPS):
    """Raw RGB frames of ``path`` from ``at`` for ``length`` seconds."""
    return [*_decoder(path, at), "-t", f"{max(0.0, length):.3f}", "-map", "0:v:0", "-an", "-sn",
            "-vf", f"fps={fps},scale={size[0]}:{size[1]}", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]


def frame_command(path, at, size):
    """One raw RGB frame of ``path`` at ``at``."""
    return [*_decoder(path, at), "-map", "0:v:0", "-an", "-sn", "-frames:v", "1",
            "-vf", f"scale={size[0]}:{size[1]}", "-f", "rawvideo", "-pix_fmt", "rgb24", "pipe:1"]


def audio_command(path, at, length, tracks):
    """16-bit stereo sound of ``tracks`` (mixed when several), or None for silence."""
    if not tracks:
        return None
    argv = ["ffmpeg", "-hide_banner", "-v", "error", "-nostdin", "-ss", f"{max(0.0, at):.3f}", "-i", str(path),
            "-t", f"{max(0.0, length):.3f}", "-vn", "-sn"]
    if len(tracks) > 1:
        inputs = "".join(f"[0:a:{t}]" for t in tracks)
        argv += ["-filter_complex", f"{inputs}amix=inputs={len(tracks)}:normalize=0[a]", "-map", "[a]"]
    else:
        argv += ["-map", f"0:a:{tracks[0]}"]
    return argv + ["-ac", "2", "-ar", str(AUDIO_RATE), "-f", "s16le", "pipe:1"]


def clock(seconds):
    seconds = max(0.0, seconds)
    minutes, rest = divmod(seconds, 60)
    return f"{int(minutes)}:{rest:05.2f}"


def ppm(size, rgb):
    """A frame as Tk reads it without any image library."""
    return b"P6 %d %d 255\n" % size + rgb


def _start(argv):
    from ..clipedit import _executable
    from ..host import child_options

    return subprocess.Popen(_executable(argv), stdin=subprocess.DEVNULL, stdout=subprocess.PIPE,
                            stderr=subprocess.DEVNULL, **child_options())


def _still(path, at, size):
    from ..clipedit import _executable
    from ..host import child_options

    result = subprocess.run(_executable(frame_command(path, at, size)), capture_output=True, timeout=30,
                            **child_options())
    frame = result.stdout[:size[0] * size[1] * 3]
    if len(frame) != size[0] * size[1] * 3:
        # Asking for the very end can land after the last frame: show the one before.
        if at > 0.2:
            return _still(path, at - 0.2, size)
        detail = result.stderr.decode(errors="replace").strip().splitlines()
        raise RuntimeError("This clip's picture could not be decoded" + (f": {detail[-1][:200]}" if detail else ""))
    return frame


class Playback:
    """One part of the timeline playing: a decoder for the picture, one for the sound."""

    def __init__(self, path, at, length, size, tracks, engine):
        self.size, self.frames = size, queue.Queue(maxsize=FRAME_QUEUE)
        self.stopped, self.started = threading.Event(), threading.Event()
        self.video, self.sound = _start(video_command(path, at, length, size)), None
        if tracks and engine:
            try:
                self.sound = _start(audio_command(path, at, length, tracks))
            except (OSError, ValueError):
                pass   # the picture plays without its sound
        threading.Thread(target=self.read_video, daemon=True, name="jarvis-clip-picture").start()
        if self.sound:
            threading.Thread(target=self.play_sound, args=(engine,), daemon=True, name="jarvis-clip-sound").start()

    def read_video(self):
        size = self.size[0] * self.size[1] * 3
        try:
            while not self.stopped.is_set():
                frame = self.video.stdout.read(size)
                if len(frame) < size:
                    break
                while not self.stopped.is_set():
                    try:
                        self.frames.put(frame, timeout=0.1)
                        break
                    except queue.Full:
                        continue
        except (OSError, ValueError):
            pass
        while not self.stopped.is_set():
            try:
                self.frames.put(None, timeout=0.1)   # the end of this part
                return
            except queue.Full:
                continue

    def play_sound(self, engine):
        stream = None
        try:
            stream = engine.open(format=engine.pa.paInt16, channels=2, rate=AUDIO_RATE, output=True,
                                 frames_per_buffer=1024)
            first = self.sound.stdout.read(AUDIO_CHUNK)
            self.started.wait()   # with the first picture, so the two start together
            data = first
            while data and not self.stopped.is_set():
                stream.write(data)
                data = self.sound.stdout.read(AUDIO_CHUNK)
        except Exception:
            pass   # a clip without working sound still plays its picture
        finally:
            if stream is not None:
                try:
                    stream.stop_stream()
                    stream.close()
                except Exception:
                    pass

    def stop(self):
        self.stopped.set()
        self.started.set()
        for process in (self.video, self.sound):
            if process is not None and process.poll() is None:
                process.kill()
            if process is not None:
                for pipe in (process.stdout,):
                    try:
                        pipe.close()
                    except (OSError, ValueError):
                        pass


class _Engine:
    """PortAudio, started once when sound first plays and ended with the player."""

    def __init__(self):
        import pyaudiowpatch as pa

        self.pa = pa
        self.audio = pa.PyAudio()
        self.lock = threading.Lock()

    def open(self, **options):
        with self.lock:
            return self.audio.open(**options)

    def close(self):
        with self.lock:
            self.audio.terminate()


class Player:
    """The picture, a timeline, and play/step/mark controls for the clip being edited."""

    def __init__(self, window, card, work, tracks, on_mark):
        self.window, self.work, self.tracks, self.on_mark = window, work, tracks, on_mark
        self.edit = None
        self.position = 0.0
        self.playing = None   # the Playback running, while playing
        self.engine = None
        self.generation = 0
        self.still_timer = None   # a drag waiting to rest
        self.resume = False   # playing when the drag began
        self.moving = False
        self.size = (PREVIEW_WIDTH, PREVIEW_HEIGHT)
        self.blank = tk.PhotoImage(width=PREVIEW_WIDTH, height=PREVIEW_HEIGHT)
        self.image = self.blank
        self.screen = tk.Label(card, image=self.blank, background="#000000", borderwidth=0,
                               width=PREVIEW_WIDTH, height=PREVIEW_HEIGHT)
        window.wide(card, self.screen)
        self.screen.bind("<Button-1>", lambda _e: self.toggle())
        self.place = tk.DoubleVar(value=0.0)
        self.scale = ttk.Scale(card, from_=0.0, to=1.0, orient="horizontal", variable=self.place,
                               command=self.dragged)
        window.wide(card, self.scale)
        controls = ttk.Frame(card, style="Card.TFrame", borderwidth=0)
        self.play_button = ttk.Button(controls, text="Play", width=8, command=self.toggle, style="Primary.TButton")
        self.play_button.pack(side="left", padx=(0, 8))
        for label, command in (("−5 s", lambda: self.step(-5)), ("−1 frame", lambda: self.step_frame(-1)),
                               ("+1 frame", lambda: self.step_frame(1)), ("+5 s", lambda: self.step(5))):
            ttk.Button(controls, text=label, width=0, command=command).pack(side="left", padx=(0, 6))
        self.time = tk.StringVar(value="Open a clip to watch it here.")
        ttk.Label(controls, textvariable=self.time, style="Card.TLabel").pack(side="left", padx=(10, 0))
        window.wide(card, controls)
        window.buttons(card, [("Mark start here", lambda: self.mark("start")),
                              ("Mark end here", lambda: self.mark("end"))])

    # The clip ------------------------------------------------------------

    def load(self, edit, keep_position=False):
        """A clip opened, or its sections changed: show the frame at the same place."""
        self.pause()
        self.edit = edit
        first = edit.sources[edit.segments[0].source]
        self.size = preview_size(first.video)
        self.position = min(self.position, edit.duration) if keep_position else 0.0
        self.set_scale()
        self.show_still()

    def duration(self):
        return self.edit.duration if self.edit else 0.0

    def set_scale(self):
        self.moving = True
        try:
            self.scale.configure(to=max(self.duration(), 0.001))
            self.place.set(self.position)
        finally:
            self.moving = False
        total = self.duration()
        self.time.set(f"{clock(self.position)} / {clock(total)}" if self.edit else "Open a clip to watch it here.")

    def show(self, frame, size):
        self.image = tk.PhotoImage(data=ppm(size, frame), format="ppm")
        self.screen.configure(image=self.image)

    def show_still(self):
        """The frame at the current place."""
        if not self.edit:
            return
        self.generation += 1
        generation, size = self.generation, self.size
        index, at = self.edit.locate(self.position)
        path = self.edit.sources[self.edit.segments[index].source].path
        self.work(lambda: _still(path, at, size), lambda outcome: self.still_shown(generation, size, outcome))

    def still_shown(self, generation, size, outcome):
        frame, error = outcome
        if generation != self.generation or self.playing is not None:
            return
        if error:
            self.window.say(error, error=True)
            return
        self.show(frame, size)

    # Controls ------------------------------------------------------------

    def toggle(self):
        if self.playing is not None:
            self.pause()
        else:
            self.play()

    def play(self):
        if not self.edit or self.playing is not None:
            return
        if self.position >= self.duration() - 0.05:
            self.position = 0.0
        if self.engine is None:
            try:
                self.engine = _Engine()
            except Exception:
                self.engine = False   # no sound here: the picture still plays
        self.play_button.configure(text="Pause")
        self.generation += 1
        self.begin(self.position, self.generation)

    def begin(self, at, generation):
        """Play the part of the timeline that holds ``at``, up to that part's end."""
        index, source_at = self.edit.locate(at)
        segment = self.edit.segments[index]
        source = self.edit.sources[segment.source]
        tracks = [t for t in self.tracks() if t < len(source.audio)]
        try:
            playback = Playback(source.path, source_at, segment.end - source_at, self.size, tracks,
                                self.engine or None)
        except (OSError, ValueError, RuntimeError) as error:
            self.pause()
            self.window.say(f"The clip could not be played: {error}", error=True)
            return
        if self.playing is not None:
            self.playing.stop()
        playback.timeline = (at, at + (segment.end - source_at))   # this part's place in the timeline
        playback.clock = None   # set by the first frame shown
        playback.shown = 0
        self.playing = playback
        self.window.root.after(TICK_MS, lambda: self.tick(generation))

    def tick(self, generation):
        playback = self.playing
        if playback is None or generation != self.generation:
            return
        start, end = playback.timeline
        frame, ended = None, False
        now = time.monotonic()
        while True:
            due = start + playback.shown / PREVIEW_FPS
            if playback.clock is not None and due > start + (now - playback.clock):
                break
            try:
                item = playback.frames.get_nowait()
            except queue.Empty:
                break
            if item is None:
                ended = True
                break
            frame = item
            playback.shown += 1
            if playback.clock is None:   # the picture starts now, and the sound with it
                playback.clock = now
                playback.started.set()
        if frame is not None:
            self.show(frame, playback.size)
        if playback.clock is not None:
            self.position = min(end, start + (now - playback.clock))
        if ended:   # this part is over: on to the next section, or the end
            playback.stop()
            self.playing = None
            if playback.clock is None and end - start > 0.2:   # not one frame from a whole part
                self.pause()
                self.window.say("This clip's picture could not be decoded here; try Open in Windows player.",
                                error=True)
                return
            if end < self.duration() - 0.01:
                self.position = end
                self.set_scale()
                self.begin(end, generation)
                return
            self.position = self.duration()
            self.set_scale()
            self.pause()
            return
        self.set_scale()
        self.window.root.after(TICK_MS, lambda: self.tick(generation))

    def pause(self):
        if self.playing is not None:
            self.generation += 1   # its next tick stops
            self.playing.stop()
            self.playing = None
        self.play_button.configure(text="Play")

    def seek(self, position):
        if not self.edit:
            return
        was_playing = self.playing is not None
        self.pause()
        self.position = min(max(0.0, position), self.duration())
        self.set_scale()
        if was_playing:
            self.play()
        else:
            self.show_still()

    def dragged(self, value):
        """The timeline is being dragged: follow it, and decode once it rests."""
        if self.moving or not self.edit:
            return
        if self.playing is not None:
            self.pause()
            self.resume = True
        self.position = min(max(0.0, float(value)), self.duration())
        self.time.set(f"{clock(self.position)} / {clock(self.duration())}")
        if self.still_timer:
            self.window.root.after_cancel(self.still_timer)
        self.still_timer = self.window.root.after(150, self.settled)

    def settled(self):
        self.still_timer = None
        if self.resume:
            self.resume = False
            self.play()
        else:
            self.show_still()

    def step(self, seconds):
        self.seek(self.position + seconds)

    def step_frame(self, frames):
        if not self.edit:
            return
        from ..clipedit import _frame_seconds

        index, _ = self.edit.locate(self.position)
        self.seek(self.position + frames * _frame_seconds(self.edit.sources[self.edit.segments[index].source]))

    def mark(self, which):
        if self.edit:
            self.on_mark(which, self.position)

    def close(self):
        self.pause()
        if self.still_timer:
            self.window.root.after_cancel(self.still_timer)
            self.still_timer = None
        if self.engine:
            self.engine.close()
        self.engine = None
