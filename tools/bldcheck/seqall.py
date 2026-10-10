"""seq.py over every view of a set: python seqall.py <dir> <views.json>"""
import sys, os, json, subprocess
here = os.path.dirname(__file__)
d, vf = sys.argv[1], sys.argv[2]
for v in json.load(open(vf)):
    n = (v.get('seq') or {}).get('n', 12)
    subprocess.run([sys.executable, os.path.join(here, 'seq.py'), d, v['id'], str(n)], check=False)
