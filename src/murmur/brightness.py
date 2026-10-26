"""Linux DDC/CI brightness, addressed by monitor identity rather than display order."""
from __future__ import annotations
import re
import shutil
import subprocess
from .commands import CommandError


def query(*args):
    program = shutil.which('ddcutil')
    if not program:
        raise CommandError('Install ddcutil and enable DDC/CI on your monitors to control brightness')
    try:
        result = subprocess.run([program, *args], capture_output=True, text=True, timeout=12, check=False)
    except subprocess.TimeoutExpired as exc:
        raise CommandError('Monitor control timed out; check DDC/CI and I2C permissions') from exc
    if result.returncode:
        raise CommandError('Monitor control failed: ' + (result.stderr.strip() or result.stdout.strip())[:300])
    return result.stdout


def parse_detect(text):
    monitors = []
    for block in re.split(r'(?m)^(?:Display \d+|Invalid display)\s*$', text)[1:]:
        bus = re.search(r'I2C bus:\s*/dev/i2c-(\d+)\s*$', block, re.M)
        identity = re.search(r'Monitor:\s*([^\n]+)', block)
        if not bus or not identity:
            continue
        parts = [p.strip() for p in identity[1].split(':', 2)]
        if len(parts) != 3 or not parts[0] or not parts[1]:
            continue
        manufacturer, model, serial = parts
        stable = bool(serial.strip('0') and serial.lower() not in ('unspecified', 'unknown'))
        identity = 'edid:' + ':'.join(parts) if stable else f'bus:{bus[1]}:{manufacturer}:{model}'
        monitors.append({'id': identity, 'bus': int(bus[1]), 'model': model, 'serial': serial,
                         'stable': stable, 'label': f'{manufacturer} {model} ({serial or "no serial"})'})
    return monitors


def discover():
    monitors = parse_detect(query('detect', '--brief'))
    counts = {m['id']: sum(other['id'] == m['id'] for other in monitors) for m in monitors}
    for monitor in monitors:
        monitor['ambiguous'] = counts[monitor['id']] > 1
    return monitors


def read(bus):
    output = query('getvcp', '10', '--terse', '--bus', str(bus))
    m = re.fullmatch(r'\s*VCP 10 C (\d+) (\d+)\s*', output)
    if not m or not 0 <= int(m[1]) <= int(m[2]) <= 65535 or int(m[2]) == 0:
        raise CommandError('This monitor does not report a usable brightness control')
    return int(m[1]), int(m[2])


def inventory():
    monitors = discover()
    for monitor in monitors:
        try:
            current, maximum = read(monitor['bus'])
            monitor['percent'] = (100 * current + maximum // 2) // maximum
        except (OSError, CommandError) as exc:
            monitor['error'] = str(exc)
    return monitors


def execute(intent, config):
    identity = intent.target['monitor']
    if identity not in config['brightness']['monitors']:
        raise CommandError('Monitor is not configured')
    matches = [m for m in discover() if m['id'] == identity]
    if len(matches) != 1:
        raise CommandError('Monitor is disconnected or its identity is ambiguous; refresh Displays')
    monitor = matches[0]
    current, maximum = read(monitor['bus'])
    percent = intent.target.get('percent')
    if intent.action != 'brightness_set':
        delta = config['brightness']['step'] * (-1 if intent.action == 'brightness_down' else 1)
        percent = max(0, min(100, (current * 100 + maximum // 2) // maximum + delta))
    if type(percent) is not int or not 0 <= percent <= 100:
        raise CommandError('Brightness must be a whole percentage from zero to 100')
    raw = (maximum * percent + 50) // 100
    query('setvcp', '10', str(raw), '--bus', str(monitor['bus']))
    name = config['brightness']['monitors'][identity]['name']
    return f'{name} brightness set to {percent}%'
