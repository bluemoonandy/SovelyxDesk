"""Validate compiled/packaged identity without changing legacy user storage."""
import json
import re
import struct
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
brand = json.loads((ROOT / 'brand/brand.json').read_text(encoding="utf-8"))
assert brand['confirmed'] is True
assert brand['updates_ready'] is False
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
print('PASS: confirmed product identity, installer metadata, legacy namespaces, 7 ICO sizes')
