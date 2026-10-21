# Security boundaries

Jarvis runs with the desktop user's permissions. Speech recognition can be
wrong: do not add destructive actions without a separate confirmation design.
Current actions are restricted to desktop app launch/focus, workspaces, window
state, volume and preconfigured screen recording.

- Spoken text is parsed into allowlisted intents; it is not passed to a shell.
- Application aliases select discovered desktop entries. Desktop entries and
  their installed executables are trusted as part of the user's environment.
- Local IPC uses an owned 0700 directory, a 0600 Unix socket and same-UID peer
  checks. Other processes running as that same user can control the service.
- Microphone consent is required. Muting terminates the voice/capture worker;
  screen-recording audio has its own independent lifetime and settings.
- Recognition audio/transcripts are not written to disk. Recorder output is
  intentional user data. Review error logs before sharing them.
- Model downloads use fixed sources, hashes, size limits and bounded extraction.
  Speech libraries are native third-party dependencies; hashes do not replace
  dependency updates and review.

## Reporting

Use the repository's **Security → Report a vulnerability** option when it is
available. If private reporting is unavailable, open an issue requesting a
private contact without including vulnerability details. Do not put private
recordings, credentials or exploitable details into a public bug report.
