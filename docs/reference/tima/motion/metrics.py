import sys,collections
R=collections.defaultdict(list);last=None
for l in open(sys.argv[1]):
    p=l.strip().split(',')
    if p[0]=='csv': last=p[1]; R[last].append([float(p[4]),float(p[5]),float(p[6])])
    elif p[0]=='deep' and last=='fan-insert': R['fan-insert-deep'].append([float(x) for x in p[1:4]])
def m(v):
    s,f=v[0],v[-1];amp=abs(f-s)
    if amp<1: return f"{s:.1f}->{f:.1f} (still)"
    over=max((x-f)*(1 if f>s else -1) for x in v)/amp*100
    rest=next(i for i in range(len(v)) if all(abs(x-f)<=0.05*amp for x in v[i:]))
    cross=sum(1 for i in range(1,len(v)) if (v[i-1]-f)*(v[i]-f)<0)
    return f"{s:.1f}->{f:.1f} over {over:.0f}% x{cross} rest {rest*1000/60:.0f}ms"
for a,rows in R.items():
    if 'fan' not in a: continue
    for j,n in enumerate(('x','y','ang')): print(a,n,m([r[j] for r in rows]))
