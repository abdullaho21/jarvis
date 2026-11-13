"""Native Windows clip sound selection. Exports copies through bundled FFmpeg."""
from pathlib import Path
import threading
import tkinter as tk
from tkinter import filedialog, ttk

from .. import clipedit
from .encoding import MIXED_TITLE


def clip_row(path, stat, folder):
    """A library row: the clip, its game (the folder Jarvis sorted it into), when, how big."""
    from datetime import datetime

    game = path.parent.name if path.parent != folder else ""
    size = stat.st_size / 1048576
    return (path.name, game, datetime.fromtimestamp(stat.st_mtime).strftime("%Y-%m-%d %H:%M"),
            f"{size / 1024:.1f} GB" if size >= 1024 else f"{size:.0f} MB")


class ClipsPanel:
    def __init__(self, window, page):
        self.window = window
        self.source = None
        self.edit = None
        self.export = None
        self.loading = False
        self.generation = 0
        self.tracks = []
        self.touched = set()
        self.files = []
        self.scanning = False
        self.opened_once = False   # the newest clip opens by itself the first time the page is shown
        self.exported = None
        # Watching and editing come first: pick a clip right above the player.
        self.tabs = window.tabs(page, [("edit", "Watch and edit"), ("library", "Library"), ("sound", "Sound")])
        self.tabs.on_show = lambda key: self.refresh() if key == "library" else None

        pick = window.group(self.tabs["edit"], "Clip", "Your recordings and replays, newest first. Pick one to watch "
                            "and edit it, or open any video file.")
        self.recent = tk.StringVar(value="")
        self.recent_paths = {}
        self.recent_choice = ttk.Combobox(pick, textvariable=self.recent, state="readonly", width=70, values=())
        self.recent_choice.bind("<<ComboboxSelected>>", lambda _e: self.open_recent())
        window.wide(pick, self.recent_choice)
        window.buttons(pick, [("Open clip…", self.choose, "Primary.TButton"), ("Newest clip", self.open_newest),
                              ("Refresh list", self.refresh), ("Open recordings folder", window.open_folder)])

        library = window.group(self.tabs["library"], "Clips", "Your saved recordings and replays. Double-click one to "
                               "watch and edit it. Already mixed sound cannot be separated.")
        window.buttons(library, [("Open clip…", self.choose), ("Refresh recordings", self.refresh),
                                 ("Open recordings folder", window.open_folder)])
        self.library = ttk.Treeview(library, columns=("clip", "game", "saved", "size"), show="headings", height=8,
                                    selectmode="browse")
        for column, title, width, anchor in (("clip", "Clip · newest first", 330, "w"), ("game", "Game", 140, "w"),
                                             ("saved", "Saved", 130, "w"), ("size", "Size", 80, "e")):
            self.library.heading(column, text=title, anchor=anchor)
            self.library.column(column, width=width, stretch=column == "clip", anchor=anchor)
        window.wide(library, self.library)   # the card's whole width
        self.library.bind("<Double-1>", self.open_selected)
        self.library.bind("<Return>", self.open_selected)
        window.buttons(library, [("Watch and edit", self.open_selected, "Primary.TButton")])

        editor = window.group(self.tabs["sound"], "Sound tracks",
                              "Clips have their own tracks when Separate audio tracks is on (Capture › Recording audio). "
                              "\"All sound\" holds every sound together: untick a sound and it is left out of the mix.")
        self.name = tk.StringVar(value="Open a clip to choose its tracks.")
        window.line(editor, ttk.Label(editor, textvariable=self.name, style="Card.TLabel", wraplength=520))
        window.buttons(editor, [("Keep all", lambda: self.keep_all(True)), ("Mute all", lambda: self.keep_all(False))])
        self.track_box = ttk.Frame(editor, style="Card.TFrame")
        window.line(editor, self.track_box)
        self.track_box.columnconfigure(0, weight=1)
        self.mix = tk.BooleanVar(value=False)
        window.row(editor, "Mix kept tracks into one", ttk.Checkbutton(editor, variable=self.mix),
                   "Leave off to keep the selected tracks separate.")

        from .player import Player, preview_tracks
        watch = window.group(self.tabs["edit"], "Watch",
                             "Plays the clip as it will be exported: removed sections are skipped. Click the picture "
                             "to play or pause; drag the timeline to find a moment, then mark it.")
        self.player = Player(window, watch, lambda run, done: self.work(run, done),
                             lambda: preview_tracks(self.source.audio if self.source else [], self.selection()),
                             self.marked)
        cuts = window.group(self.tabs["edit"], "Sections",
                            "Start and end are seconds in the timeline above. Keep section keeps only them; Remove "
                            "section cuts them out.")
        self.start, self.end = tk.StringVar(value="0"), tk.StringVar(value="0")
        window.row(cuts, "Start (seconds)", ttk.Entry(cuts, textvariable=self.start, width=12))
        window.row(cuts, "End (seconds)", ttk.Entry(cuts, textvariable=self.end, width=12))
        window.buttons(cuts, [("Keep section", lambda: self.change("keep")),
                              ("Remove section", lambda: self.change("remove")),
                              ("Split at start", lambda: self.change("split")), ("Undo", lambda: self.change("undo"))])
        window.buttons(cuts, [("Join clip…", self.join), ("Open in Windows player", self.preview)])
        self.sections = tk.StringVar(value="")
        window.line(cuts, ttk.Label(cuts, textvariable=self.sections, style="Caption.TLabel", wraplength=520))
        output = window.group(self.tabs["edit"], "Export copy", "The original is never changed.")
        self.mode = tk.StringVar(value="Fast (keyframes)")
        window.row(output, "Cut mode", ttk.Combobox(output, textvariable=self.mode,
                   values=("Fast (keyframes)", "Exact (re-encode)"), state="readonly", width=24),
                   "Fast copies video and may begin at an earlier keyframe. Exact re-encodes on the graphics card, "
                   "in the clip's own format.")
        self.progress = tk.StringVar(value="")
        window.line(output, ttk.Label(output, textvariable=self.progress, style="Caption.TLabel", wraplength=520))
        window.buttons(output, [("Export copy", self.export_copy, "Primary.TButton"), ("Cancel export", self.cancel),
                                ("Watch the copy", self.watch_copy)])

    def work(self, run, done):
        # Probing and exports have their own worker; settings saves stay responsive.
        def worker():
            try:
                outcome = run(), None
            except Exception as error:
                outcome = None, str(error)
            self.window.post((done, outcome, None))
        threading.Thread(target=worker, name="jarvis-clip-edit", daemon=True).start()

    def shown(self):
        """The Clips page opened: list the clips again (new ones appear), and the
        first time, open the newest so the player has something to show."""
        self.refresh(open_newest=not self.opened_once)
        self.opened_once = True

    def refresh(self, open_newest=False):
        if self.scanning:
            return
        self.scanning = True
        folder = Path(self.window.variable("recording", "output_dir").get()).expanduser()
        def scan():
            if not folder.is_dir():
                return []
            files = [(p, p.stat()) for p in folder.rglob("*") if p.is_file()
                     and p.suffix.lower() in (".mp4", ".mkv", ".webm", ".mov") and not p.name.startswith(".")]
            return sorted(files, key=lambda f: f[1].st_mtime, reverse=True)[:200]
        def done(outcome):
            self.scanning = False
            found, error = outcome
            if error:
                self.window.say(error, error=True)
                return
            self.files = [path for path, _ in found]
            self.library.delete(*self.library.get_children())
            self.recent_paths = {}
            for i, (path, stat) in enumerate(found):
                row = clip_row(path, stat, folder)
                self.library.insert("", "end", iid=str(i), values=row)
                self.recent_paths[" · ".join(part for part in row if part)] = path
            self.recent_choice.configure(values=list(self.recent_paths))
            if not found:
                self.library.insert("", "end", iid="empty", values=(
                    "No clips yet. Say \"clip that\" during a game, or Open clip… for another folder.", "", "", ""))
                self.recent.set("No clips in your recordings folder yet: use Open clip…")
            else:
                self.show_open_clip()
            if open_newest and found and self.source is None and not self.loading:
                self.open(found[0][0])
        self.work(scan, done)

    def show_open_clip(self):
        """The picker names the clip that is open (or asks for one)."""
        if not self.source:
            self.recent.set("Pick a clip to watch and edit…")
            return
        opened = Path(self.source.path)
        shown = next((label for label, path in self.recent_paths.items() if Path(path).resolve() == opened), None)
        self.recent.set(shown or opened.name)

    def open_recent(self):
        path = self.recent_paths.get(self.recent.get())
        if path:
            self.open(path)

    def open_newest(self):
        if self.files:
            self.open(self.files[0])
        else:
            self.refresh(open_newest=True)

    def watch_copy(self):
        if self.exported and Path(self.exported).is_file():
            self.open(self.exported)
        else:
            self.window.say("Export a copy first.")

    def open_selected(self, _event=None):
        selected = self.library.selection()
        if selected and selected[0].isdigit():
            self.open(self.files[int(selected[0])])

    def choose(self):
        path = filedialog.askopenfilename(parent=self.window.root, title="Open a clip",
                filetypes=[("Video clips", "*.mp4 *.mkv *.webm *.mov"), ("All files", "*.*")])
        if path:
            self.open(path)

    def open(self, path):
        if self.loading or self.export is not None:
            self.window.say("Wait for the clip to open, or finish or cancel the export first.")
            return
        self.loading = True
        self.name.set("Opening " + Path(path).name + "…")
        self.work(lambda: clipedit.probe(path), self.loaded)

    def loaded(self, outcome):
        self.loading = False
        source, error = outcome
        if error:
            self.name.set(Path(self.source.path).name if self.source else "Open a clip to choose its tracks.")
            self.window.say(error, error=True)
            return
        self.source = source
        self.edit = clipedit.Edit.of(source)
        self.generation += 1
        generation = self.generation
        self.name.set(f"{Path(source.path).name} · {source.duration:.1f} seconds")
        self.show_open_clip()
        self.start.set("0")
        self.end.set(str(source.duration))
        self.edited()
        self.tabs.show("edit")
        self.progress.set("Checking for silent tracks…" if source.audio else "This clip has no sound tracks.")
        for widget in self.track_box.winfo_children():
            widget.destroy()
        self.tracks, self.touched = [], set()
        for i, track in enumerate(source.audio):
            keep = tk.BooleanVar(value=True)
            details = tk.StringVar(value="Checking sound…")
            self.tracks.append((keep, details))
            title = f"{track['name']} · {track.get('codec_name', 'audio')} · {track.get('channels', '?')} channels · {track.get('sample_rate', '?')} Hz"
            ttk.Checkbutton(self.track_box, text=title, variable=keep,
                            command=lambda i=i: self.ticked(i)).grid(row=i * 2, column=0, sticky="w", pady=(6, 0))
            ttk.Button(self.track_box, text="Only this", command=lambda i=i: self.only(i)).grid(row=i * 2, column=1, padx=(12, 0))
            ttk.Label(self.track_box, textvariable=details, style="Caption.TLabel").grid(row=i * 2 + 1, column=0, sticky="w", padx=(24, 0), pady=(0, 6))
        if source.audio:
            def checked(outcome):
                if generation != self.generation:
                    return
                peaks, error = outcome
                if error:
                    for _keep, detail in self.tracks:
                        detail.set("Sound level could not be checked.")
                else:
                    for i, peak in enumerate(peaks):
                        silent = peak is not None and peak <= clipedit.SILENT_DB
                        self.tracks[i][1].set("No sound in this clip" if silent else "Sound present" if peak is not None else "Sound level unknown")
                        if silent and i not in self.touched:
                            self.tracks[i][0].set(False)
                self.progress.set("Watch it above and mark what to keep; choose its sound on the Sound tab.")
            self.work(lambda: clipedit.loudness(source.path, len(source.audio)), checked)

    def ticked(self, index):
        """Leaving a sound out of a clip whose first track is "All sound" (which
        holds that sound too) drops "All sound" and mixes the tracks kept, so the
        copy plays everything else, without it, in any player."""
        self.touched.add(index)
        audio = self.source.audio if self.source else []
        if (index and not self.tracks[index][0].get() and audio
                and audio[0].get("name") == MIXED_TITLE and self.tracks[0][0].get()):
            self.tracks[0][0].set(False)
            self.touched.add(0)
            self.mix.set(True)
            self.progress.set(f"\"{MIXED_TITLE}\" also holds {audio[index].get('name', 'that sound')}, so it is left out; "
                              "the tracks you keep are mixed into one.")

    def only(self, index):
        for i, (keep, _detail) in enumerate(self.tracks):
            keep.set(i == index)
        self.touched.update(range(len(self.tracks)))

    def keep_all(self, value):
        for keep, _detail in self.tracks:
            keep.set(value)
        self.touched.update(range(len(self.tracks)))

    def selection(self):
        return [i for i, (keep, _detail) in enumerate(self.tracks) if keep.get()]

    def export_copy(self):
        if self.source is None or self.loading or self.export is not None:
            self.window.say("Open a clip and finish or cancel any export first.")
            return
        try:
            start, end = self.marks()
            if not 0 <= start < end <= self.edit.duration:
                raise ValueError("Start and end must be within the clip, with end after start.")
            import copy
            edit = copy.deepcopy(self.edit)
            edit.trim(start, end)
            exporter = clipedit.Export(edit, "fast" if self.mode.get().startswith("Fast") else "exact",
                                       self.selection(), self.mix.get())
        except (ValueError, OSError) as error:
            self.window.say(str(error), error=True)
            return
        self.export = exporter
        self.progress.set("Exporting copy…")
        def done(outcome):
            self.export = None
            path, error = outcome
            self.progress.set("Export stopped." if error else "Saved: " + str(path) + " · Watch the copy to see it.")
            self.window.say(error if error else "Saved a new copy: " + str(path), error=bool(error))
            if not error:
                self.exported = path
                self.refresh()   # the copy appears in the list
        self.work(exporter.run, done)

    def preview(self):
        if self.source:
            import os
            os.startfile(self.source.path)

    def edited(self, keep_position=False):
        self.start.set("0")
        self.end.set(f"{self.edit.duration:.2f}")
        self.sections.set(f"{len(self.edit.sources)} source clip(s) · {len(self.edit.segments)} section(s) · {self.edit.duration:.2f} seconds")
        self.player.load(self.edit, keep_position)

    def marks(self):
        """Start and end, in seconds. Shown to the hundredth, so an end rounded up
        past the clip's end (12.346 shows as 12.35) means the end."""
        start, end = float(self.start.get()), float(self.end.get())
        duration = self.edit.duration
        return start, duration if duration < end <= duration + 0.01 else end

    def marked(self, which, seconds):
        """The player's place, as the section's start or end."""
        (self.start if which == "start" else self.end).set(f"{seconds:.2f}")
        try:
            start, end = float(self.start.get()), float(self.end.get())
        except ValueError:
            return
        if which == "start" and start >= end:   # a start past the end: the end follows, to the clip's end
            self.end.set(f"{self.edit.duration:.2f}")
        self.window.say(f"{'Start' if which == 'start' else 'End'} set to {seconds:.2f} seconds.")

    def change(self, action):
        if not self.edit or self.export is not None or self.loading:
            return
        try:
            start, end = self.marks()
            if action != "undo" and not 0 <= start < end <= self.edit.duration:
                raise ValueError("Choose a section inside the edited timeline.")
            if action == "keep": self.edit.trim(start, end)
            elif action == "remove": self.edit.remove_range(start, end)
            elif action == "split": self.edit.split(start)
            elif action == "undo": self.edit.undo()
            self.edited(keep_position=action in ("split", "undo"))
        except ValueError as error:
            self.window.say(str(error), error=True)

    def join(self):
        if not self.edit or self.loading or self.export is not None:
            return
        path = filedialog.askopenfilename(parent=self.window.root, title="Join clip at end",
                filetypes=[("Video clips", "*.mp4 *.mkv *.webm *.mov"), ("All files", "*.*")])
        if not path: return
        self.loading = True
        def done(outcome):
            self.loading = False
            source, error = outcome
            if error:
                self.window.say(error, error=True)
                return
            try:
                self.edit.append(source)
                self.edited(keep_position=True)
            except ValueError as error:
                self.window.say(str(error), error=True)
        self.work(lambda: clipedit.probe(path), done)

    def cancel(self):
        if self.export is not None:
            self.export.cancel()

    def close(self):
        self.generation += 1
        self.cancel()
        self.player.close()
