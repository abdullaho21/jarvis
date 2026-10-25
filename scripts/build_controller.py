#!/usr/bin/env python3
"""Build the Rust background controller; never changes or starts a service."""
if __package__:
    from .native_build import build
else:
    from native_build import build


def main():
    build(__doc__, [("controller", "jarvis-controller")],
          "Run it directly, or select MURMUR_CONTROLLER=rust with the existing daemon launcher.")


if __name__ == "__main__":
    main()
