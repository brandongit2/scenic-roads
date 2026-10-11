"""The field's whole-frame cost (on − off) of each view in a time.mjs result: python3 tsum.py <result.json>"""
import json
import sys

d = json.load(open(sys.argv[1]))
for k, v in d.items():
    c = [r['on']['frameMed'] - r['off']['frameMed'] for r in v['runs']]
    r0 = v['runs'][0]
    print(f"{k:18s} field " + ' / '.join(f'+{x:.2f}' for x in c) + f" ms  (on {r0['on']['frameMed']:.2f}, off {r0['off']['frameMed']:.2f}, tiles {r0['on']['tiles']})")
