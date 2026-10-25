import sys, glob, colorsys, math
import numpy as np
from PIL import Image
S=sys.argv[1]
fs=sorted(glob.glob(S+'/f/*.png'))
prev=None; rows=[]
def hue_mask(a, lo, hi, smin=0.45, vmin=0.5):
    r,g,b=[a[...,k].astype(np.float32)/255 for k in range(3)]
    mx=np.maximum(np.maximum(r,g),b); mn=np.minimum(np.minimum(r,g),b)
    d=mx-mn+1e-6; s=d/(mx+1e-6)
    h=np.where(mx==r,((g-b)/d)%6,np.where(mx==g,(b-r)/d+2,(r-g)/d+4))*60
    return (h>=lo)&(h<=hi)&(s>smin)&(mx>vmin)
for k,f in enumerate(fs):
    a=np.asarray(Image.open(f).convert('RGB'))
    H,W,_=a.shape
    fan=a[int(H*0.10):int(H*0.48), int(W*0.25):int(W*0.75)].astype(np.int16)
    e=0.0 if prev is None else float(np.abs(fan-prev).mean())
    prev=fan
    m=hue_mask(fan.astype(np.uint8), 22, 42)  # orange
    ys,xs=np.nonzero(m)
    if len(xs)>40:
        cx,cy=xs.mean(),ys.mean()
        cov=np.cov(np.vstack([xs-cx,ys-cy]))
        ang=0.5*math.degrees(math.atan2(2*cov[0,1],cov[0,0]-cov[1,1]))
        n=len(xs)
    else:
        cx=cy=ang=float('nan'); n=0
    rows.append((k,k/60,e,cx,cy,ang,n))
np.savetxt(S+'/curves.csv',np.array(rows),delimiter=',',header='frame,t,energy,orange_x,orange_y,orange_angle,orange_px',fmt='%.4f')
E=np.array([r[2] for r in rows])
# events: energy above 0.8
thr=0.6
ev=[]; on=False
for k,e in enumerate(E):
    if e>thr and not on: on=True; s=k
    if on and e<0.25:
        # require 8 quiet frames
        if all(E[k:k+8]<0.25): ev.append((s,k)); on=False
for s,e in ev:
    seg=E[s:e]; peaks=sum(1 for i in range(1,len(seg)-1) if seg[i]>seg[i-1] and seg[i]>=seg[i+1] and seg[i]>0.4)
    print(f'event {s/60:.2f}-{e/60:.2f}s  dur {(e-s)/60*1000:.0f} ms  peak energy {seg.max():.2f}  energy peaks {peaks}')
