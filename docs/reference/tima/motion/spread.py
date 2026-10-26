import sys,glob,numpy as np
from PIL import Image
fs=sorted(glob.glob(sys.argv[1]+'/f/*.png'));out=[]
for k,f in enumerate(fs):
    a=np.asarray(Image.open(f).convert('RGB')).astype(np.float32)/255
    H,W,_=a.shape;a=a[int(H*.05):int(H*.55),int(W*.2):int(W*.8)]
    mx=a.max(2);mn=a.min(2);s=(mx-mn)/(mx+1e-6);m=(s>0.3)&(mx>0.4)
    ys,xs=np.nonzero(m)
    if len(xs)<100: out.append((k,np.nan,np.nan,np.nan,0));continue
    cx,cy=xs.mean(),ys.mean();r=np.sqrt((xs-cx)**2+(ys-cy)**2)
    out.append((k,cx,cy,np.percentile(r,90),len(xs)))
np.savetxt(sys.argv[1]+'/spread.csv',np.array(out),delimiter=',',header='frame,cx,cy,r90,px',fmt='%.2f')
