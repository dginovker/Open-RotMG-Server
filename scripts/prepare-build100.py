#!/usr/bin/env python3
"""Prepare user-supplied build100 assets; never modifies or publishes the source SWF."""
import hashlib
import json
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import xml.etree.ElementTree as ET
import zipfile

SWF_SHA = 'ec031064b61d22e860de802ae1967ef1bb193b2c35d297ca1d25b1a96f76684e'
RUFFLE_SHA = '77e7b3f818dc1ac25b309fd2380b6c23223a8fcd46563061ae27ebbbb522f24b'


def prepare(swf, ffdec, ruffle, output):
    for path, expected in [(swf, SWF_SHA), (ruffle, RUFFLE_SHA)]:
        if hashlib.sha256(path.read_bytes()).hexdigest() != expected:
            raise ValueError(f'Unsupported asset checksum: {path}; expected {expected}')
    if output.exists() and any(output.iterdir()):
        raise FileExistsError(f'Output directory must be empty: {output}')
    output.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='build100-') as temp:
        subprocess.run(['java', '-Xss8m', '-jar', str(ffdec.resolve()), '-format', 'script:pcode',
                        '-selectclass', 'com.company.assembleegameclient.util.++', '-export', 'script',
                        temp, str(swf.resolve())], check=True, stdout=subprocess.DEVNULL)
        sources = [p.read_text() for p in Path(temp).rglob('*.pcode')]
        sources = [s for s in sources if 'id=\\"Wizard\\"' in s and 'id=\\"Pirate\\"' in s]
        if len(sources) != 1:
            raise ValueError(f'Expected exactly one embedded content class, found {len(sources)}')
        fragments = re.findall(r'pushstring ("(?:[^"\\]|\\.)*")', sources[0])
        joined = ''.join(json.loads(s.replace("\\'", "'")) for s in fragments)
        roots = [ET.fromstring(s) for s in re.findall(r'<Objects>.*?</Objects>', joined, re.S)]
        grounds = re.findall(r'<GroundTypes>.*?</GroundTypes>', joined, re.S)
        if len(roots) != 7 or sum(map(len, roots)) != 528 or len(grounds) != 1:
            raise ValueError('Incomplete build100 embedded XML inventory')
        objects = ET.Element('Objects')
        for root in roots:
            objects.extend(root)
        ET.ElementTree(objects).write(output / 'objects.xml', encoding='unicode')
        (output / 'grounds.xml').write_text(grounds[0])
    shutil.copyfile(swf, output / 'client.swf')
    with zipfile.ZipFile(ruffle) as archive:
        for entry in archive.infolist():
            if not (output / 'vendor' / entry.filename).resolve().is_relative_to((output / 'vendor').resolve()):
                raise ValueError('Unsafe Ruffle archive path')
        archive.extractall(output / 'vendor')
    (output / 'index.html').write_text(HTML)
    (output / 'build100.json').write_text(json.dumps({'client_sha256': SWF_SHA, 'ruffle_zip_sha256': RUFFLE_SHA,
                                                     'objects': 528, 'ground_types': 115}, indent=2) + '\n')
    print(f'Prepared unchanged build100 client and Ruffle 0.7.1-openrotmg.5 at {output}')


HTML = '''<!doctype html><meta charset="utf-8">
<meta http-equiv="Content-Security-Policy" content="default-src 'self'; script-src 'self' 'unsafe-inline' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self'; worker-src 'self' blob:; img-src 'self' data:; media-src 'self' blob:">
<title>OpenRotMG build 100 compatibility arena</title>
<style>body{background:#222;color:white;font:14px sans-serif}ruffle-player{display:block;width:800px;height:600px}pre{white-space:pre-wrap}</style>
<div id="game"></div><pre id="status">Experimental solo compatibility arena. Not a complete or historically faithful 2010 server.</pre>
<script src="/vendor/ruffle.js"></script><script>
(async()=>{
 const p=window.RufflePlayer.newest().createPlayer();document.querySelector('#game').append(p);
 const socket=new URL('/game',location.href);socket.protocol='ws:';
 await p.ruffle().load({url:'/client.swf',autoplay:'on',unmuteOverlay:'hidden',openUrlMode:'deny',allowScriptAccess:false,preferredRenderer:'wgpu-webgl',logLevel:'info',
 urlRewriteRules:[[/^http:\\/\\/www\\.realmofthemadgod\\.com\\//,location.origin+'/legacy/'],[/^http:\\/\\/(?:rotmgtesting|realmofthemadgod)\\.appspot\\.com\\//,location.origin+'/legacy/']],
 socketProxy:[{host:'127.0.0.1',port:2050,proxyUrl:socket.href}]});
})().catch(e=>{document.querySelector('#status').textContent=String(e);console.error(e)});
</script>'''

if __name__ == '__main__':
    if len(sys.argv) != 5:
        raise SystemExit('Usage: prepare-build100.py SWF FFDEC_JAR RUFFLE_0.7.1_OPENROTMG.5_ZIP OUTPUT_DIRECTORY')
    prepare(*(Path(s) for s in sys.argv[1:]))
