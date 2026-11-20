"""Validate compiled/packaged identity without changing legacy user storage."""
import json
import re
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
brand = json.loads((ROOT / 'brand/brand.json').read_text(encoding="utf-8"))
assert brand['confirmed'] is True
assert brand['update_source'] == 'https://api.github.com/repos/' + brand['repository_slug'] + '/releases'
assert brand['updates_ready'] is False
assert brand['repository_slug'] == 'bluemoonandy/SovelyxDesk'
assert brand['repository'] == 'https://github.com/' + brand['repository_slug']
assert brand['legacy_repository_slug'] == 'bluemoonandy/maccanvas'
assert brand['executable'] == 'SovelyxDesk.exe'
assert brand['legacy_data_directory'] == 'MacCanvas'
assert brand['legacy_credential_prefixes'] == ['MacCanvas/profile/', 'MacCanvas/login/v2/']
installer = (ROOT / 'install.ps1').read_text(encoding="utf-8")
embedded = re.search(r"return @'\n(.*?)\n'@ \| ConvertFrom-Json", installer, re.S)
assert embedded and json.loads(embedded[1]) == brand, 'Standalone installer brand metadata is stale'
reserved, kind, count = struct.unpack_from('<HHH', (ROOT / brand['ico']).read_bytes())
assert (reserved, kind, count) == (0, 1, 7)
entries = (ROOT / brand['ico']).read_bytes()
sizes = [(entries[6 + i * 16] or 256, entries[7 + i * 16] or 256) for i in range(count)]
assert sizes == [(s, s) for s in [16, 24, 32, 48, 64, 128, 256]]
for size in [16, 24, 32, 48, 64, 128, 256]:
    assert (ROOT / f'assets/brand/sovelyx-desk-{size}.png').is_file()
for view in ['ui/app.slint', 'ui/about.slint']:
    assert brand['icon'] in (ROOT / view).read_text(encoding='utf-8'), 'UI icon and brand configuration differ'
assert brand['release_name'] + '-${{ github.sha }}' in (ROOT / '.github/workflows/ci.yml').read_text(encoding='utf-8')
print('PASS: confirmed product identity, installer metadata, legacy namespaces, 7 ICO sizes')
