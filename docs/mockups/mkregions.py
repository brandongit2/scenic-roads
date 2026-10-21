import json,glob,math,sys,os,collections
S=sys.argv[1]; D=os.path.expanduser('~/Library/Application Support/scenic/accepted/regions/sources/inputs/regions/@drawn/')
paths={r['id']:r['path'] for r in json.load(open(os.path.expanduser('~/Library/Application Support/scenic/regions.json')))}
dl=json.load(open(S+'/downloads.json'))['regions']
sw=json.load(open(S+'/swarm.json'))['agent']; built=sw.get('built',{}); fc={r['id']:r for r in sw['forecast']['regions']}
def merc(lon,lat):
    lat=max(-85,min(85,lat)); return (lon+180)/360,(1-math.log(math.tan(math.radians(lat))+1/math.cos(math.radians(lat)))/math.pi)/2
def dp(pts,eps):
    if len(pts)<3: return pts
    (x0,y0),(x1,y1)=pts[0],pts[-1]; dmax=0;idx=0; dx,dy=x1-x0,y1-y0; L=math.hypot(dx,dy)
    for i in range(1,len(pts)-1):
        x,y=pts[i]; d=abs(dy*x-dx*y+x1*y0-y1*x0)/L if L>1e-12 else math.hypot(x-x0,y-y0)
        if d>dmax: dmax=d;idx=i
    if dmax>eps: return dp(pts[:idx+1],eps)[:-1]+dp(pts[idx:],eps)
    return [pts[0],pts[-1]]
R=lambda v: round(v,5)
def pretty(slug): return ' '.join(w.capitalize() if w not in ('and','of','the','da') else w for w in slug.replace('-',' ').split())
out=[]; tot=0
for f in sorted(glob.glob(D+'*.json')):
    d=json.load(open(f)); rid=d['id']
    segs=[]  # (a,b) in degrees
    fill=[]
    for entry,polys in d['shapes'].items():
        for poly in polys:
            ring=[(R(x),R(y)) for x,y in poly[0]]
            if ring[0]!=ring[-1]: ring.append(ring[0])
            for a,b in zip(ring,ring[1:]):
                if a!=b: segs.append((a,b))
            pts=[merc(x,y) for x,y in ring]
            xs=[p[0] for p in pts]; ys=[p[1] for p in pts]
            if (max(xs)-min(xs))<1.5e-4 and (max(ys)-min(ys))<1.5e-4: continue
            s=dp(pts,0.00018)
            if len(s)>=4: fill.append([round(v*1000,1) for xy in s for v in xy])
    # axis-aligned stretches covered twice within this region are cut lines, not borders
    groups=collections.defaultdict(list); keep=[]
    for a,b in segs:
        if a[0]==b[0]: groups[('x',a[0])].append((min(a[1],b[1]),max(a[1],b[1])))
        elif a[1]==b[1]: groups[('y',a[1])].append((min(a[0],b[0]),max(a[0],b[0])))
        else: keep.append((a,b))
    for (ax,v),iv in groups.items():
        ev=collections.Counter()
        for lo,hi in iv: ev[lo]+=1; ev[hi]-=1
        xs=sorted(ev); c=0
        for p,q in zip(xs,xs[1:]):
            c+=ev[p]
            if c==1:
                keep.append(((v,p),(v,q)) if ax=='x' else ((p,v),(q,v)))
    # chain kept segments into polylines
    adj=collections.defaultdict(list)
    for i,(a,b) in enumerate(keep): adj[a].append(i); adj[b].append(i)
    used=[False]*len(keep); lines=[]
    for i in range(len(keep)):
        if used[i]: continue
        used[i]=True; a,b=keep[i]; line=[a,b]
        for end in (1,0):
            while True:
                tip=line[-1] if end else line[0]; nxt=None
                for j in adj[tip]:
                    if not used[j]: nxt=j; break
                if nxt is None: break
                used[nxt]=True; p,q=keep[nxt]; o=q if p==tip else p
                if end: line.append(o)
                else: line.insert(0,o)
        pts=[merc(x,y) for x,y in line]; s=dp(pts,0.00018)
        xs=[p[0] for p in s]; ys=[p[1] for p in s]
        if len(s)<2 or ((max(xs)-min(xs))<1.5e-4 and (max(ys)-min(ys))<1.5e-4): continue
        lines.append([round(v*1000,1) for xy in s for v in xy])
    path=paths.get(rid,''); parts=path[:-5].split('/') if path else [rid]
    crumbs=[pretty(p) for p in parts[:-1]]+[d['name']]
    z=dl.get(rid,{}); b=built.get(rid,{}); fr=fc.get(rid,{})
    out.append({'id':rid,'name':d['name'],'crumbs':crumbs,'outline':d.get('outline',[]),'bytes':z.get('bytes'),'tiles':[b.get('built'),b.get('total')],
      'rank':fr.get('rank'),'on_map':fr.get('on_map'),'left':fr.get('left'),'ready_at':fr.get('ready_at'),'map_at':fr.get('map_at'),'fill':fill,'edge':lines})
    tot+=len(lines)
js='const REGIONS='+json.dumps(out,separators=(',',':'))+';\nconst SNAPSHOT_AT='+str(json.load(open(S+'/swarm.json'))['now'])+';'
open(S+'/regions.js','w').write(js); print(len(js), tot)
