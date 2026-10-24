"""Configured voice controls, exact brightness values, and hardware selection."""
import copy
import dataclasses
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import types
import unittest
from unittest.mock import patch

from murmur import brightness, config
from murmur.commands import CommandError, Parser, all_wake_variants, closest_match, match_candidates
from tests.test_rust_controller import BINARY, Harness, policies
from tests.test_voice_upgrade import FakeModel, FakeRecognizer, words


def settings():
    return config.validate({
        'activation': {'wake_phrase': 'alfred', 'wake_aliases': ['jarvis']},
        'aliases': {'firefox': 'firefox.desktop', 'video': 'video.desktop'},
        'app_prefixes': {'firefox': '', 'video': 'watch'},
        'brightness': {'monitors': {'edid:KTC:Main:123': {'name': 'KTC', 'aliases': ['k t c']},
                                    'edid:SAM:Second:456': {'name': 'Samsung', 'aliases': ['side screen']}},
                       'default': 'edid:KTC:Main:123', 'step': 10}})


class VoiceControlsTests(unittest.TestCase):
    def setUp(self):
        self.c = settings()
        self.parser = Parser.from_config(self.c)

    def test_bare_and_custom_launch_words_remain_allowlisted(self):
        for phrase, target in [('firefox', 'firefox'), ('watch video', 'video'), ('watch the video', 'video'),
                               ('please watch the video', 'video'), ('open the steam', 'steam'), ('focus the firefox', 'firefox')]:
            self.assertEqual(self.parser.parse(phrase).target, target)
        for phrase in ('open firefox', 'launch video', 'arbitrary executable'):
            with self.assertRaises(CommandError, msg=phrase):
                self.parser.parse(phrase)
        # Legacy settings keep all existing verbs and fixed phrases.
        self.assertEqual(Parser().parse('open the video').target, 'video')
        self.assertEqual(Parser().parse('open the video recorder').action, 'open_recorder')

    def test_exact_brightness_values_aliases_and_default(self):
        for phrase in ['set KTC brightness to 67%', 'set k t c brightness to sixty-seven percent',
                       'brighten KTC to sixty seven', 'dim the KTC to 67 percent', 'set brightness to 67']:
            intent = self.parser.parse(phrase)
            self.assertEqual(intent.action, 'brightness_set')
            self.assertEqual(intent.target, {'monitor': 'edid:KTC:Main:123', 'percent': 67})
        self.assertEqual(self.parser.parse('dim side screen').target['monitor'], 'edid:SAM:Second:456')
        self.assertEqual(self.parser.parse('brighten').action, 'brightness_up')
        candidates = match_candidates(self.parser.grammar(['firefox', 'video', 'steam']))
        self.assertIn('firefox', candidates)
        self.assertNotIn('open firefox', candidates)
        for phrase in ['set KTC brightness to -67', 'set KTC brightness to +67', 'set KTC brightness to −67', 'set KTC brightness to 6.7',
                       'set KTC brightness to 101', 'set unknown brightness to 67', 'set KTC brightness to sixty eightish']:
            with self.assertRaises(CommandError, msg=phrase):
                self.parser.parse(phrase)
            self.assertIsNone(closest_match(phrase, candidates), phrase)
        self.assertIsNone(closest_match('fire fox', candidates, exact_only=True))
        result = closest_match('set KTC brightness to 67 percent', candidates, exact_only=True)
        self.assertIsNotNone(result)
        self.assertEqual(self.parser.parse(result[0]).target['percent'], 67)

    def test_invalid_configuration_cannot_disable_startup(self):
        patches = [ {'activation': {'wake_aliases': ['hey Alfred']}}, {'activation': {'wake_aliases': 'jarvis'}},
                    {'app_prefixes': {'missing': ''}},
                    {'aliases': {'listening': 'a'}, 'app_prefixes': {'listening': 'stop'}},
                    {'brightness': {'default': 'missing'}}, {'brightness': {'step': True}},
                    {'brightness': {'monitors': {'a': {'name': 'KTC', 'aliases': []}, 'b': {'name': 'ktc', 'aliases': []}}}} ]
        for update in patches:
            case = config.merged(self.c, update)
            with self.assertRaises(ValueError, msg=update):
                config.validate(case)
        self.assertEqual(all_wake_variants(self.c['activation']), ['alfred', 'jarvis'])
        self.assertEqual(config.validate({})['activation']['wake_aliases'], [])

    def test_each_wake_name_can_carry_a_bare_app_command(self):
        from murmur.vosk_engine import VoskEngine
        from murmur import models  # Import before patch.dict restores sys.modules.
        fake = types.SimpleNamespace(Model=FakeModel, KaldiRecognizer=FakeRecognizer, SetLogLevel=lambda _: None)
        with patch.dict(sys.modules, vosk=fake), patch.object(models, 'valid_model', return_value=True):
            engine = VoskEngine(self.parser.grammar(['firefox', 'video']), 'alfred', model_path='/model', wake_aliases=['jarvis'])
        FakeRecognizer.script = []
        for wake in ('alfred', 'jarvis'):
            engine.begin_wake()
            self.assertIsNotNone(engine._wake_result(words(wake + ' firefox')))
            self.assertEqual(engine.after_wake().text, 'firefox')
        engine.begin_wake()
        self.assertIsNone(engine._wake_result(words('firefox')))
        self.assertIsNone(engine._wake_result(words('someone over there mentioned jarvis firefox')))
        engine.close()


DETECT = '''Display 1
   I2C bus: /dev/i2c-10
   Monitor: KTC:Main:123
Display 2
   I2C bus: /dev/i2c-16
   Monitor: SAM:Second:456
'''


class BrightnessBackendTests(unittest.TestCase):
    def test_reordered_buses_scale_percent_and_verify_with_ddcutil(self):
        c = settings()
        log = []
        def query(*args):
            log.append(args)
            if args[0] == 'detect':
                return DETECT.replace('i2c-10', 'i2c-29')
            if args[0] == 'getvcp':
                return 'VCP 10 C 128 255\n'
            return ''
        with patch('murmur.brightness.query', side_effect=query):
            result = brightness.execute(Parser.from_config(c).parse('set KTC brightness to 67'), c)
        self.assertIn('67%', result)
        self.assertEqual(log[-1], ('setvcp', '10', '171', '--bus', '29'))
        self.assertNotIn('--noverify', log[-1])

    def test_disconnected_duplicate_and_unsupported_displays_never_write(self):
        for detect, read in [('', 'VCP 10 C 50 100'), (DETECT + DETECT, 'VCP 10 C 50 100'),
                             (DETECT, 'VCP 10 ERR'), (DETECT, 'VCP 10 C 0 0')]:
            log = []
            def query(*args):
                log.append(args)
                return detect if args[0] == 'detect' else read
            with patch('murmur.brightness.query', side_effect=query), self.assertRaises(CommandError):
                brightness.execute(Parser.from_config(settings()).parse('dim KTC'), settings())
            self.assertFalse(any(call[0] == 'setvcp' for call in log))

    def test_relative_change_clamps_and_invalid_display_is_visible(self):
        log = []
        def query(*args):
            log.append(args)
            return DETECT if args[0] == 'detect' else 'VCP 10 C 98 100'
        with patch('murmur.brightness.query', side_effect=query):
            brightness.execute(Parser.from_config(settings()).parse('brighten KTC'), settings())
        self.assertEqual(log[-1][2], '100')
        displays = brightness.parse_detect(DETECT.replace('Display 2', 'Invalid display').replace('123', '000000'))
        self.assertEqual(len(displays), 2)
        self.assertFalse(displays[0]['stable'])


@unittest.skipUnless(BINARY.is_file(), 'build the Rust controller')
class RustVoiceControlsTests(unittest.TestCase):
    def test_parser_and_grammar_match_python_for_configured_controls(self):
        c = settings()
        parser = Parser.from_config(c)
        grammar = parser.grammar(['firefox', 'video', 'steam'])
        phrases = grammar['phrases'] + ['watch the video', 'please watch the video', 'open the steam',
                'set ktc brightness to 67%', 'brighten KTC to sixty seven', 'dim Samsung to zero',
                'set KTC brightness to -67', 'set KTC brightness to 6.7', 'set KTC brightness to 101', 'open firefox']
        actual = policies([{'op': 'parse', 'text': p, 'config': c} for p in phrases])
        for phrase, result in zip(phrases, actual, strict=True):
            try:
                expected = dataclasses.asdict(parser.parse(phrase))
            except ValueError:
                self.assertFalse(result['ok'], phrase)
            else:
                self.assertEqual(result.get('result', {}).get('intent'), expected, (phrase, result))
        actual = policies([{'op': 'grammar', 'apps': ['firefox', 'video', 'steam'], 'config': c}])[0]
        self.assertEqual(actual['result']['grammar'], grammar)

    def test_real_rust_dispatch_uses_selected_monitor_and_keeps_status_responsive(self):
        h = Harness(config_patch=settings(), dry_run=False)
        self.addCleanup(h.close)
        script = h.bin / 'ddcutil'
        script.write_text('#!/usr/bin/env python3\n' + '''import os,sys,json,time
from pathlib import Path
args=sys.argv[1:]
with Path(os.environ['MURMUR_TEST_LOG']).open('a') as f: f.write(json.dumps({'program':'ddcutil','args':args})+'\\n')
if args[0]=='detect':
 if (Path(os.environ['MURMUR_TEST_LOG']).parent/'slow').exists(): time.sleep(1)
 print(''' + repr(DETECT.replace('i2c-10', 'i2c-29')) + ''')
elif args[0]=='getvcp': print('VCP 10 C 128 255')
''')
        script.chmod(0o755)
        result = h.rpc('brightness_monitors')
        self.assertTrue(result['ok'], result)
        self.assertEqual(len(result['monitors']), 2)
        result = h.rpc('execute', phrase='set KTC brightness to 67')
        self.assertTrue(result['ok'], result)
        self.assertEqual(h.events('ddcutil')[-1]['args'], ['setvcp', '10', '171', '--bus', '29'])
        self.assertTrue(h.rpc('status')['ok'])
        import threading
        import time
        (h.root / 'slow').touch()
        response = {}
        job = threading.Thread(target=lambda: response.update(h.rpc('brightness_monitors')))
        job.start()
        self.addCleanup(lambda: job.join(timeout=5))
        h.wait(lambda: sum(e['args'][0] == 'detect' for e in h.events('ddcutil')) >= 3)
        start = time.monotonic()
        self.assertTrue(h.rpc('mute', muted=True)['ok'])
        self.assertLess(time.monotonic() - start, .5)
        job.join(timeout=5)
        self.assertTrue(response.get('ok'), response)


@unittest.skipUnless(os.environ.get('MURMUR_VOSK_MODEL') and os.environ.get('MURMUR_SPEECH_TEST_RAW'), 'provide public audio and local Vosk model')
class RealWakeAliasTests(unittest.TestCase):
    def test_secondary_wake_name_and_same_breath_command_match_python(self):
        from murmur.native_worker import prepare
        from murmur.vosk_engine import VoskEngine
        from murmur.voice import rms
        c = settings()
        c['activation']['wake_aliases'] = ['go forward']
        c['commands'] = {'go forward ten meters': 'cancel', 'ten meters': 'cancel'}
        c['recognition'].update(accurate=False, model_path=os.environ['MURMUR_VOSK_MODEL'])
        grammar = Parser.from_config(c).grammar(['firefox', 'video'])
        fixture = os.environ['MURMUR_SPEECH_TEST_RAW']
        audio = Path(fixture).read_bytes()
        audio += bytes(((len(audio) + 64000 + 959) // 960) * 960 - len(audio))
        engine = VoskEngine(grammar, 'alfred', model_path=c['recognition']['model_path'], wake_aliases=['go forward'])
        engine.begin_wake()
        expected = []
        for offset in range(0, len(audio), 960):
            frame = audio[offset:offset + 960]
            event = engine.feed(frame, quiet=rms(frame) < .005)
            if event:
                expected.append(event.to_dict())
                if follow := engine.after_wake():
                    expected.append(follow.to_dict())
                break
        engine.close()
        self.assertEqual([e['kind'] for e in expected], ['wake', 'command'])
        with patch.dict(os.environ, MURMUR_VOICE_WORKER='rust'):
            argv, payload, env = prepare(c, 'once', grammar)
        request = json.dumps(payload) + '\n' + json.dumps({'path': fixture, 'session': 'wake'}) + '\n'
        out = subprocess.run(argv + ['--engine'], input=request, env=env, text=True, capture_output=True, timeout=30, check=True)
        events = [json.loads(line) for line in out.stdout.splitlines()][1]['events']
        self.assertEqual(events, expected)
