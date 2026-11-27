# Convergence: threads from the left pinch into one beam on the right (after an
# earlier cover illustration), drawn straight on the dot grid so each thread is a line of dots.
# Writes convergence.png here; run to_levels.py on it afterwards, as for gen.py's arts.
import numpy as np
from PIL import Image
W,H=2400,1500; cell=4; gw,gh=W//cell,H//cell
rng=np.random.default_rng(11)
acc=np.zeros((gh,gw))
fx,fy=gw*0.80,gh*0.50  # centred like the cover: fan is symmetric above and below the beam
N=420; S=2400; t=np.linspace(0,1,S)
for i in range(N):
    x0=rng.uniform(-0.10,0.30)*gw; y0=fy+np.clip(rng.normal(0,0.55),-1.4,1.4)*gh
    p=rng.uniform(2.4,3.4); e=1-(1-t)**p
    wob=np.sin(t*np.pi*rng.uniform(1,3)+rng.uniform(0,6.3))*rng.normal(0,0.012)*gh*(1-t)
    xi=(x0+(fx-x0)*t).astype(int); yi=(y0+(fy-y0)*e+wob).astype(int)
    m=(xi>=0)&(xi<gw)&(yi>=0)&(yi<gh)
    cells=np.unique(yi[m]*gw+xi[m])  # one hit per cell per thread
    w=0.55+0.45*(xi[m].astype(float)/gw)
    np.add.at(acc.ravel(),cells,np.interp(cells%gw,[0,gw],[0.55,1.0]))
yy,xx=np.mgrid[0:gh,0:gw].astype(float)
thr=1-np.exp(-acc*0.9)
right=1/(1+np.exp(np.clip(-(xx-fx)/3,-50,50)))
beam=(np.abs(yy-fy)<0.6)*right*1.0+np.exp(-((yy-fy)/4)**2)*right*0.35
glow=np.exp(-(((xx-fx)/55)**2+((yy-fy)/22)**2))*0.6
f=np.clip(thr*0.75+beam+glow,0,1)*np.clip(0.1+xx/(gw*0.45),0,1)
f=np.clip(f+rng.normal(0,0.03,f.shape),0,1)
def bayer(n=8):
    m=np.array([[0,2],[3,1]])
    while m.shape[0]<n: m=np.block([[4*m,4*m+2],[4*m+3,4*m+1]])
    return (m+0.5)/m.size
th=np.tile(bayer(),(gh//8+1,gw//8+1))[:gh,:gw]
lvl=np.where(f>th,0.35+0.65*f,0.0)
d=3; mask=np.zeros((cell,cell)); mask[0:d,0:d]=1
big=np.kron(lvl,mask)
base=np.array([24,24,26.]); ink=np.array([236,236,236.])
out=np.zeros((H,W,3)); out[:]=base; out[:big.shape[0],:big.shape[1]]=base+(ink-base)*big[...,None]
im=Image.fromarray(out.clip(0,255).astype(np.uint8)); im.save('convergence.png')
print('ok')
