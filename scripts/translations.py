"""Single catalogue -> bundled Slint PO + Rust presentation strings. No network/runtime files."""
import argparse,json,re,subprocess
from pathlib import Path
ROOT=Path(__file__).resolve().parent.parent
STRING=r'"(?:\\.|[^"\\])*"'
def sources():
    result=set()
    for p in (ROOT/'ui').glob('*.slint'):
        for m in re.finditer(r'@tr\("ui"\s*=>\s*('+STRING+')',p.read_text()):result.add(json.loads(m.group(1)))
    return result

def generated(catalog,language):
    header='msgid ""\nmsgstr ""\n"Content-Type: text/plain; charset=UTF-8\\n"\n"Language: '+language+'\\n"\n"Plural-Forms: nplurals=1; plural=0;\\n"\n\n'
    if language=='en':header=header.replace('nplurals=1; plural=0','nplurals=2; plural=(n != 1)')
    for source in sorted(catalog):
        header+='msgctxt "ui"\nmsgid '+json.dumps(source,ensure_ascii=False)+'\nmsgstr '+json.dumps(catalog[source] if language=='zh_CN' else source,ensure_ascii=False)+'\n\n'
    return header

def main():
    parser=argparse.ArgumentParser();parser.add_argument('--check',action='store_true');parser.add_argument('--extract',action='store_true');args=parser.parse_args()
    catalog=json.loads((ROOT/'translations/zh_CN.json').read_text())
    missing=sources()-catalog.keys()
    if missing:raise SystemExit('Missing UI translations: '+json.dumps(sorted(missing),ensure_ascii=False))
    for source,target in catalog.items():
        if not target:raise SystemExit('Empty translation: '+source)
        fields=lambda s:sorted(re.findall(r'\{([^{}]+)\}',s))
        if fields(source)!=fields(target):raise SystemExit('Placeholder mismatch: '+source)
    for language in ['en','zh_CN']:
        path=ROOT/'translations'/language/'LC_MESSAGES/maccanvas.po';expected=generated(catalog,language)
        if args.check:
            if path.read_text()!=expected:raise SystemExit('Stale translation resource: '+str(path))
        else:path.parent.mkdir(parents=True,exist_ok=True);path.write_text(expected)
    if args.extract:
        # The pinned official extractor is optional for authoring, never needed by the EXE.
        subprocess.run(['slint-tr-extractor','--output',str(ROOT/'translations/maccanvas.pot'),*[str(p) for p in (ROOT/'ui').glob('*.slint')]],check=True)
    print(f'PASS: {len(sources())} Slint sources, {len(catalog)} bilingual entries; PO and placeholders verified')
if __name__=='__main__':main()
