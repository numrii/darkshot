"""Make Darkshot's emoji list (src/emoji_list.txt) from Unicode's emoji-test.txt and
GitHub gemoji's emoji.json (shortcodes and tags).

    python scripts/emoji_list.py emoji-test.txt src/emoji_list.txt emoji.json

emoji-test.txt: https://unicode.org/Public/emoji/latest/emoji-test.txt
emoji.json:     https://raw.githubusercontent.com/github/gemoji/master/db/emoji.json

Keeps fully-qualified emoji, leaves out the Component group and skin-tone variants,
and writes one group header (`@Name`) followed by `emoji name|shortcodes|tags` lines.
"""
import json
import sys

src, out, gemoji = sys.argv[1], sys.argv[2], sys.argv[3]
plain = lambda e: e.replace(chr(0xFE0F), '')
codes = {plain(x['emoji']): (x['aliases'], x['tags']) for x in json.load(open(gemoji, encoding='utf-8'))}
SKIN = {chr(c) for c in range(0x1F3FB, 0x1F400)}
lines = []
version = ""
group = None
count = 0
for raw in open(src, encoding='utf-8'):
    line = raw.rstrip('\n')
    if line.startswith('# Version:'):
        version = line.split(':', 1)[1].strip()
    if line.startswith('# group:'):
        group = line.split(':', 1)[1].strip()
        if group != 'Component':
            lines.append('@' + group)
        continue
    if not line or line.startswith('#') or group == 'Component':
        continue
    data, _, comment = line.partition('#')
    if 'fully-qualified' not in data:
        continue
    # comment: " 😀 E1.0 grinning face"
    parts = comment.strip().split(' ', 2)
    emoji, name = parts[0], parts[2]
    if any(c in SKIN for c in emoji):
        continue
    aliases, tags = codes.get(plain(emoji), ([], []))
    extra = f"|{' '.join(aliases)}|{' '.join(tags)}".rstrip('|')
    lines.append(f'{emoji} {name}{extra}')
    count += 1

header = [
    f'# Emoji from Unicode emoji-test.txt, version {version}: fully qualified, without skin-tone',
    '# variants, with shortcodes and tags from GitHub gemoji. Generated: one "@Group" line per',
    '# group, then "emoji name|shortcodes|tags" lines.',
    '# Names: (c) Unicode, Inc., used under the Unicode License v3: https://www.unicode.org/license.txt',
    '# Shortcodes and tags: (c) 2019 GitHub, Inc., MIT License: https://github.com/github/gemoji',
]
open(out, 'w', encoding='utf-8', newline='\n').write('\n'.join(header + lines) + '\n')
print(count, 'emoji')
