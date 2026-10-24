"""Exact monitor commands and configured application launch words."""
from __future__ import annotations
import re


def number(n):
    small = 'zero one two three four five six seven eight nine ten eleven twelve thirteen fourteen fifteen sixteen seventeen eighteen nineteen'.split()
    if n < 20:
        return small[n]
    if n == 100:
        return 'one hundred'
    tens = ['', '', 'twenty', 'thirty', 'forty', 'fifty', 'sixty', 'seventy', 'eighty', 'ninety']
    return tens[n // 10] + (' ' + small[n % 10] if n % 10 else '')


def numeric(text):
    return ' '.join(number(int(w)) if w.isascii() and w.isdigit() and int(w) <= 100 else w for w in text.split())


def brightness_like(text):
    return bool({'brightness', 'brighten', 'dim'} & set(text.split()))


def invalid_number(text):
    return bool(re.search(r'[+−–—﹣－＋-]\s*\d|\d[.,]\d|\b(minus|negative|point)\b', text, re.I))


class Rules:
    def __init__(self, launches=None, monitors=None):
        self.launches = launches or {}
        self.monitors = monitors or {}

    @classmethod
    def from_config(cls, c):
        from .commands import normalize
        rules = cls()
        prefixes = c['app_prefixes']
        if not isinstance(prefixes, dict) or len(prefixes) > 256:
            raise ValueError('Invalid application prefixes')
        aliases = {normalize(k): v for k, v in c['aliases'].items()}
        seen = set()
        for alias, prefix in prefixes.items():
            name = normalize(alias)
            if name in seen or not isinstance(prefix, str) or len(prefix) > 40 or any(ord(ch) < 32 or ord(ch) == 127 for ch in prefix) or len(prefix.split()) > 4:
                raise ValueError('Invalid or duplicate application prefix')
            seen.add(name)
            if name not in aliases:
                raise ValueError('Launch words need an existing application alias')
            p = normalize(prefix)
            phrase = f'{p} {name}' if p else name
            if brightness_like(phrase) or phrase in rules.launches:
                raise ValueError('Conflicting application shortcut')
            rules.launches[phrase] = name
        b = c['brightness']
        if not isinstance(b, dict) or set(b) != {'monitors', 'default', 'step'} or not isinstance(b['monitors'], dict):
            raise ValueError('Invalid brightness settings')
        if len(b['monitors']) > 8 or not isinstance(b['default'], str) or (b['default'] and b['default'] not in b['monitors']):
            raise ValueError('Choose a configured default monitor (up to eight monitors)')
        if type(b['step']) is not int or not 1 <= b['step'] <= 100:
            raise ValueError('Brightness step must be 1–100')
        names = set()
        for identity, monitor in b['monitors'].items():
            if not isinstance(identity, str) or not identity or len(identity.encode()) > 512 or any(ord(ch) < 32 or ord(ch) == 127 for ch in identity) or not isinstance(monitor, dict) or set(monitor) != {'name', 'aliases'} or not isinstance(monitor['aliases'], list):
                raise ValueError('Invalid monitor settings')
            for value in [monitor['name'], *monitor['aliases']]:
                if not isinstance(value, str) or len(value) > 40 or any(ord(ch) < 32 or ord(ch) == 127 for ch in value):
                    raise ValueError('Choose distinct short monitor names without command words')
                n = numeric(normalize(value))
                if not n or n in names or {'brightness', 'brighten', 'dim', 'percent', 'to', 'by', 'the', 'main'} & set(n.split()):
                    raise ValueError('Choose distinct short monitor names without command words')
                names.add(n)
                rules.monitors[n] = identity
        if len(names) > 8:
            raise ValueError('Use at most eight monitor names and aliases in total')
        if b['default']:
            rules.monitors['main'] = b['default']
        return rules

    def parse(self, text):
        from .commands import Intent, normalize
        p = normalize(text)
        if p in self.launches:
            return Intent('open_app', self.launches[p])
        for shortcut, alias in self.launches.items():
            prefix = shortcut[:-len(alias)]
            if p == prefix + 'the ' + alias:
                return Intent('open_app', alias)
        if not brightness_like(p):
            return None
        p = numeric(p)
        for verb in ('brighten', 'dim', 'set'):
            p = p.replace(verb + ' the ', verb + ' ', 1)
        for name, identity in sorted(self.monitors.items()):
            for action in ('brighten', 'dim'):
                if p == f'{action} {name}' or (name == 'main' and p == action):
                    return Intent('brightness_down' if action == 'dim' else 'brightness_up', {'monitor': identity})
            for n in range(101):
                forms = [f'set {name} brightness to {number(n)}', f'brighten {name} to {number(n)}', f'dim {name} to {number(n)}']
                if name == 'main':
                    forms.append(f'set brightness to {number(n)}')
                if any(p == f or p == f + ' percent' for f in forms):
                    return Intent('brightness_set', {'monitor': identity, 'percent': n})
        return None

    def phrases(self):
        result = list(self.launches)
        for name in self.monitors:
            result.extend((f'brighten {name}', f'dim {name}'))
            result.extend(f'set {name} brightness to {number(n)} percent' for n in range(101))
        return sorted(result)

    def canonical(self, text):
        intent = self.parse(text)
        if intent is None:
            return None
        if intent.action == 'open_app':
            return next(p for p, name in sorted(self.launches.items()) if name == intent.target)
        name = next(n for n, identity in sorted(self.monitors.items()) if identity == intent.target['monitor'])
        if intent.action == 'brightness_set':
            return f"set {name} brightness to {number(intent.target['percent'])} percent"
        return ('brighten ' if intent.action == 'brightness_up' else 'dim ') + name
