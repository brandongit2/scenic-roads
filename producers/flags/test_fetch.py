"""The symbols' producer (fetch.py): the free list read as the check reads it, the shape it writes."""
import os
import re
import tempfile
import tomllib
import unittest
import zipfile

import fetch

SYMBOLS_RS = os.path.join(os.path.dirname(__file__), '..', '..', 'crates', 'inputs', 'src', 'inputs', 'regions', 'symbols.rs')


class FreeList(unittest.TestCase):
    def test_matched_word_by_word_as_the_check_does(self):
        lines = fetch.free_list()
        for l in ['Public domain', 'CC0', 'CC BY 4.0', 'CC BY-SA 3.0', 'CC BY 2.5 ar', 'Copyrighted free use', 'OGL-om 1.0']:
            self.assertTrue(fetch.free(l, lines), l)
        for l in ['CC BY-NC 4.0', 'CC0 1.0', 'CC BY 4', 'CC BY 2.5 AR', 'public domain', 'GFDL', '', 'CC BY-SA 4.0 ar x']:
            self.assertFalse(fetch.free(l, lines), l)

    def test_the_size_bound_is_the_checks(self):
        with open(SYMBOLS_RS, encoding='utf-8') as f:
            m = re.search(r'pub const MAX_BYTES: u64 = (\d+) << (\d+);', f.read())
        self.assertEqual(int(m.group(1)) << int(m.group(2)), fetch.MAX_BYTES)


class Shape(unittest.TestCase):
    def test_declarations_gain_a_table_and_the_bundle_is_the_same_bytes_each_time(self):
        with tempfile.TemporaryDirectory() as d:
            src, out = os.path.join(d, 'regions'), os.path.join(d, 'out')
            os.makedirs(os.path.join(src, 'canada'))
            os.makedirs(os.path.join(src, 'how'))
            files = {
                'canada/_group.toml': 'name = "Canada"',
                'canada/alberta.toml': '# its border\nid = "alberta"\nname = "Alberta"\noutline = ["osm:391186"]\n',
                'how/example.toml': 'id = "x"\n',
                '_pins.toml': 'osm = "2026-09-29"\n',
            }
            for p, t in files.items():
                with open(os.path.join(src, p), 'w') as f:
                    f.write(t)
            decls = fetch.declarations(src)
            self.assertEqual(sorted(decls), ['canada/_group.toml', 'canada/alberta.toml'])
            s = {'image': 'Q1.svg', 'kind': 'flag', 'source': 'https://example.org/File:"A".svg', 'author': '', 'licence': 'CC0'}
            g = dict(s, image='Q16.png', kind='coat of arms', author='Ann "B" Ç')
            fetch.write(out, decls, {'canada/alberta.toml': s, 'canada/_group.toml': g}, {'Q1.svg': b'<svg/>', 'Q16.png': b'\x89PNG'})
            with open(os.path.join(out, 'canada/alberta.toml'), encoding='utf-8') as f:
                text = f.read()
            self.assertTrue(text.startswith(files['canada/alberta.toml']))
            self.assertEqual(tomllib.loads(text)['symbol'], {k: v for k, v in s.items() if v})
            with open(os.path.join(out, 'canada/_group.toml'), encoding='utf-8') as f:
                self.assertEqual(tomllib.loads(f.read())['symbol']['author'], 'Ann "B" Ç')
            with zipfile.ZipFile(os.path.join(out, 'symbols.zip')) as z:
                self.assertEqual(z.namelist(), ['Q1.svg', 'Q16.png'])
            first = open(os.path.join(out, 'symbols.zip'), 'rb').read()
            self.assertEqual(fetch.bundle({'Q16.png': b'\x89PNG', 'Q1.svg': b'<svg/>'}), first)
            with open(os.path.join(out, 'originals.sha256')) as f:
                self.assertEqual([l.split()[1] for l in f], ['canada/_group.toml', 'canada/alberta.toml'])
            # Placed, then run again: the symbol kept, the bundle's member still named kept.
            for p in ['canada/alberta.toml', 'symbols.zip']:
                os.replace(os.path.join(out, p), os.path.join(src, p))
            decls = fetch.declarations(src)
            self.assertEqual(decls['canada/alberta.toml'][1]['symbol']['image'], 'Q1.svg')
            self.assertEqual(fetch.kept_images(src, decls), {'Q1.svg': b'<svg/>'})

    def test_an_image_is_named_by_its_bytes(self):
        self.assertEqual(fetch.kind_of(b'\x89PNG\r\n\x1a\nxx'), 'png')
        self.assertEqual(fetch.kind_of(b'\xff\xd8\xff\xe0'), 'jpg')
        self.assertEqual(fetch.kind_of(b'<?xml'), 'svg')


if __name__ == '__main__':
    unittest.main()
