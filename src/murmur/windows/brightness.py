"""Laptop WMI and external-monitor DDC/CI through screen_brightness_control."""

import hashlib
from ..commands import CommandError
from .system import com_context


def _discover():
    import screen_brightness_control as sbc

    result = []
    for item in sbc.list_monitors_info():
        serial, edid = str(item.get("serial") or ""), str(item.get("edid") or "")
        stable = bool(
            serial and serial.strip("0") and serial.lower() not in ("unknown", "none")
        )
        identity = (
            "win-edid:" + hashlib.sha256(edid.encode()).hexdigest()
            if edid
            else f"win-monitor:{item.get('manufacturer')}:{item.get('model')}:{serial}:{item['index']}"
        )
        result.append(
            {
                "id": identity,
                "label": str(item.get("name", "Monitor")),
                "model": str(item.get("model", "")),
                "serial": serial,
                "stable": stable,
                "index": item["index"],
                "method": item["method"].__name__.lower(),
            }
        )
    for monitor in result:
        monitor["ambiguous"] = sum(m["id"] == monitor["id"] for m in result) != 1
    return result


def inventory():
    import screen_brightness_control as sbc

    with com_context():
        result = _discover()
        for item in result:
            try:
                item["percent"] = int(
                    sbc.get_brightness(display=item["index"], method=item["method"])[0]
                )
            except Exception:
                item["error"] = (
                    "Brightness is unavailable; enable DDC/CI for an external monitor"
                )
        return result


def execute(intent, config):
    import screen_brightness_control as sbc

    identity = intent.target["monitor"]
    if identity not in config["brightness"]["monitors"]:
        raise CommandError("Monitor is not configured")
    with com_context():
        matches = [m for m in _discover() if m["id"] == identity]
        if len(matches) != 1:
            raise CommandError("Monitor is disconnected or ambiguous; refresh Displays")
        monitor = matches[0]
        try:
            percent = intent.target.get("percent")
            if intent.action != "brightness_set":
                current = sbc.get_brightness(
                    display=monitor["index"], method=monitor["method"]
                )[0]
                delta = config["brightness"]["step"] * (
                    -1 if intent.action == "brightness_down" else 1
                )
                percent = max(0, min(100, current + delta))
            if type(percent) is not int or not 0 <= percent <= 100:
                raise CommandError(
                    "Brightness must be a whole percentage from zero to 100"
                )
            sbc.set_brightness(
                percent, display=monitor["index"], method=monitor["method"]
            )
        except CommandError:
            raise
        except Exception as exc:
            raise CommandError(
                "Monitor brightness failed; check DDC/CI and Windows display permissions"
            ) from exc
    return f"{config['brightness']['monitors'][identity]['name']} brightness set to {percent}%"
