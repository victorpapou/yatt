import sys,glob,math,numpy as np
from PIL import Image
S=sys.argv[1]; fs=sorted(glob.glob(S+'/f/*.png'))
H={'yellow':(58,75),'green':(95,150),'purple':(275,305),'pink':(330,352)}
def hue(a):
    r,g,b=[a[...,k].astype(np.float32)/255 for k in range(3)]
    mx=np.maximum(np.maximum(r,g),b);mn=np.minimum(np.minimum(r,g),b);d=mx-mn+1e-6
    h=np.where(mx==r,((g-b)/d)%6,np.where(mx==g,(b-r)/d+2,(r-g)/d+4))*60
    return h,d/(mx+1e-6),mx
prev=None;rows=[]
for k,f in enumerate(fs):
    a=np.asarray(Image.open(f).convert('RGB'));h,s,v=hue(a)
    e=0 if prev is None else float(np.abs(a.astype(np.int16)-prev).mean());prev=a.astype(np.int16)
    row=[k,k/60,e]
    for n,(lo,hi) in H.items():
        m=(h>=lo)&(h<=hi)&(s>0.35)&(v>0.45);ys,xs=np.nonzero(m)
        if len(xs)>60:
            cx,cy=xs.mean(),ys.mean();c=np.cov(np.vstack([xs-cx,ys-cy]))
            row+=[cx,cy,0.5*math.degrees(math.atan2(2*c[0,1],c[0,0]-c[1,1])),len(xs)]
        else: row+=[np.nan]*3+[0]
    rows.append(row)
hdr='frame,t,energy,'+','.join(f'{n}_x,{n}_y,{n}_angle,{n}_px' for n in H)
np.savetxt(S+'/curves.csv',np.array(rows),delimiter=',',header=hdr,fmt='%.4f')
