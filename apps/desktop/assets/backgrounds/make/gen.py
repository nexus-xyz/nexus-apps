# Draws the dot-matrix arts into the current folder (a <name>.png and a scratch <name>-thumb.png each). Only
# eclipse, lattice, orbits, moire and horizon ship, with convergence.py's convergence; contours, deepfield and pulsar
# do not, and the -thumb files are for looking at the output. Run to_levels.py on each shipped art afterwards: the app
# embeds the grey level maps it writes.
import numpy as np
from PIL import Image
W,H=2400,1500  # main pane ~1140x840 at 2x-ish
rng=np.random.default_rng(7)
yy,xx=np.mgrid[0:H,0:W].astype(np.float32)
u=(xx-W/2)/H; v=(yy-H/2)/H

def bayer(n=8):
    m=np.array([[0,2],[3,1]])
    while m.shape[0]<n:
        m=np.block([[4*m,4*m+2],[4*m+3,4*m+1]])
    return (m+0.5)/m.size
B=bayer(8)

def dither(f, cell=4, dot=0.75, base=(24,24,26), ink=(236,236,236), grain=0.035):
    """f: 0..1 field; render as a grid of square dots (dot-matrix), lit when f > bayer threshold"""
    h,w=f.shape
    gh,gw=h//cell,w//cell
    small=f[:gh*cell,:gw*cell].reshape(gh,cell,gw,cell).mean(axis=(1,3))
    small=np.clip(small+rng.normal(0,grain,small.shape),0,1)
    th=np.tile(B,(gh//8+1,gw//8+1))[:gh,:gw]
    on=small>th
    # intensity of lit dots varies a bit with field for depth
    lvl=np.where(on, 0.35+0.65*np.clip(small,0,1), 0.0)
    img=np.zeros((h,w,3),np.float32); img[:]=base
    d=max(1,int(cell*dot))
    mask=np.zeros((cell,cell),np.float32); o=(cell-d)//2; mask[o:o+d,o:o+d]=1
    big=np.kron(lvl,mask)
    ink=np.array(ink,np.float32); base_=np.array(base,np.float32)
    img[:gh*cell,:gw*cell]=base_+(ink-base_)*big[...,None]
    return Image.fromarray(img.clip(0,255).astype(np.uint8))

def save(name,f,**k):
    im=dither(np.clip(f,0,1),**k); im.save(f'./{name}.png'); im.resize((480,300),Image.LANCZOS).save(f'./{name}-thumb.png')

# 1 Eclipse: planet limb lower right with corona glow
cx,cy,R=0.55,0.95,0.95
r=np.sqrt((u-cx)**2+(v-cy)**2)
limb=np.exp(-((r-R)/0.006)**2)*1.0
corona=np.exp(-np.clip(r-R,0,None)/0.22)*0.7*(r>R)
inside=(r<R)*np.clip(0.10*np.exp(-(R-r)/0.08),0,1)
stars=(rng.random((H,W))>0.9993)*(r>R)*0.9
ang=np.arctan2(v-cy,u-cx); dirf=np.clip(np.cos(ang+2.2),0,1)**1.5  # brighter toward upper-left
save('eclipse',(limb*dirf*1.2+corona*(0.3+0.7*dirf)+inside+stars)-0.2*(r<R-0.01))

# 2 Wireframe sphere (latitude/longitude grid), right side
cx,cy,R=0.42,0.08,0.62
dx,dy=(u-cx)/R,(v-cy)/R; rr=dx**2+dy**2; m=rr<1
z=np.sqrt(np.clip(1-rr,0,1))
# rotate sphere
t=0.5; X=dx*np.cos(t)+z*np.sin(t); Z=-dx*np.sin(t)+z*np.cos(t); p=0.45; Y=dy*np.cos(p)-Z*np.sin(p); Z=dy*np.sin(p)+Z*np.cos(p)
lat=np.arcsin(np.clip(Y,-1,1)); lon=np.arctan2(X,Z)
g1=np.exp(-(np.sin(lat*9)**2)/0.004); g2=np.exp(-(np.sin(lon*9)**2)/0.004*np.cos(lat)**2)
shade=0.25+0.75*np.clip(-0.5*dx-0.6*dy+0.6*z,0,1)
rim=np.exp(-((np.sqrt(rr)-1)/0.01)**2)
save('lattice',(np.maximum(g1,g2)*m*shade*0.95+rim*0.7+m*0.05*shade))

# 3 Orbits: concentric tilted ellipses + small bodies
f=np.zeros_like(u); cx,cy=0.35,0.25
for i,a in enumerate([0.25,0.42,0.62,0.86,1.15,1.5]):
    e=np.sqrt(((u-cx)/a)**2+((v-cy)/(a*0.32))**2)
    f+=np.exp(-((e-1)/ (0.004/a))**2)*(0.9-0.1*i)
    th=i*1.3+0.6; px,py=cx+a*np.cos(th),cy+a*0.32*np.sin(th)
    f+=np.exp(-((u-px)**2+(v-py)**2)/(0.0004+0.0002*i))*1.2
f+=np.exp(-((u-cx)**2+(v-cy)**2)/0.004)*1.0+np.exp(-((u-cx)**2+(v-cy)**2)/0.05)*0.25
f+=(rng.random((H,W))>0.9994)*0.8
save('orbits',f)

# 4 Topographic contours from value noise
def vnoise(scale,seed):
    r=np.random.default_rng(seed); gs=int(W/scale)+3; gt=int(H/scale)+3
    g=r.random((gt,gs))
    xs=xx/scale; ys=yy/scale; x0=xs.astype(int); y0=ys.astype(int); fx=xs-x0; fy=ys-y0
    sx=fx*fx*(3-2*fx); sy=fy*fy*(3-2*fy)
    a=g[y0,x0]; b=g[y0,x0+1]; c=g[y0+1,x0]; d=g[y0+1,x0+1]
    return a+(b-a)*sx+(c-a)*sy+(a-b-c+d)*sx*sy
n=vnoise(900,1)*0.7+vnoise(450,2)*0.3
lv=n*14; c=np.abs(lv-np.round(lv)); lines=np.exp(-(c/0.035)**2)
fade=np.clip(0.25+0.9*(xx/W)*(1-0.4*yy/H),0,1)
save('contours',lines*fade*0.9)

# 5 Moiré: two rotated line gratings -> interference
def grating(ang,period,cx,cy):
    return 0.5+0.5*np.cos(2*np.pi*(((xx-cx)*np.cos(ang)+(yy-cy)*np.sin(ang))/period))
rad1=np.sqrt((xx-W*0.7)**2+(yy-H*0.3)**2); rad2=np.sqrt((xx-W*0.78)**2+(yy-H*0.36)**2)
mo=(0.5+0.5*np.cos(rad1/9))*(0.5+0.5*np.cos(rad2/9))
env=np.exp(-((xx-W*0.74)**2+(yy-H*0.33)**2)/(2*(W*0.33)**2))
save('moire',mo*env*1.1)

# 6 Deep field: stars + soft band (milky way style)
band=np.exp(-((v-(-0.35*u+0.05))/0.11)**2)
cloud=np.clip(vnoise(220,9)*0.55+vnoise(70,10)*0.45-0.15,0,1)*1.4
st=(rng.random((H,W))>0.9985)*1.0+(rng.random((H,W))>0.9996)*1.5
save('deepfield',band*cloud*0.75+st*(0.4+band),cell=4)
print('ok')


from PIL import ImageDraw, ImageFilter
def canvas(): 
    im=Image.new('L',(W,H),0); return im, ImageDraw.Draw(im)
# 7 Pulsar ridgelines with occlusion
im,dr=canvas()
rows=40; top=H*0.16; bot=H*0.94; step=(bot-top)/rows
xs=np.linspace(W*0.30,W*0.98,700); xn=(xs-W*0.30)/(W*0.68)
for i in range(rows):
    y0=top+i*step; r2=np.random.default_rng(300+i)
    prof=np.zeros_like(xn)
    for k in range(7):
        c=0.5+0.13*r2.normal(); w=0.012+0.035*r2.random(); prof+=r2.random()*np.exp(-((xn-c)/w)**2)
    env=np.exp(-((xn-0.5)/0.2)**4)
    ys=y0-(prof*env*150+r2.normal(0,1.6,len(xn))*(0.25+env))
    pts=list(zip(xs,ys)); poly=pts+[(xs[-1],H),(xs[0],H)]
    dr.polygon(poly,fill=0)
    dr.line(pts,fill=int(120+120*(i/rows)**0.3),width=4)
f=np.asarray(im.filter(ImageFilter.GaussianBlur(1.2)),np.float32)/255
save('pulsar',f*1.15,cell=4)

# 8 Horizon grid
im,dr=canvas(); hz=H*0.60; vx=W*0.5
for k in range(-40,41):
    xb=vx+k*300; dr.line([(vx+k*4,hz),(xb,H*1.6)],fill=220,width=5)
for j in range(1,40):
    y=hz+ (H*0.42)*(1/(1+ (40-j)*0.35))*1.0
    y=hz+ (H)*(0.9/(j*0.55+0.6))**-1*0.02*j*j/ (1) 
for j in range(0,30):
    z=1+j*0.6; y=hz+ (H*0.55)/z*0+ (H-hz)*(1/z)
    dr.line([(0,H-(y-hz)+0),(0,0)],fill=0)
im2=Image.new('L',(W,H),0); d2=ImageDraw.Draw(im2)
for j in range(1,12):
    z=j*0.9; y=hz+(H-hz)*1.1/(z+0.1)
    if y<H+5 and y>hz: d2.line([(0,y),(W,y)],fill=220,width=5)
g=np.maximum(np.asarray(im,np.float32),np.asarray(im2,np.float32))/255
yy_=np.arange(H)[:,None]; fade=np.clip((yy_-hz)/(H*0.25),0,1)**1.2
g=g*fade*(yy_>hz)
glow=np.exp(-((yy_-hz)/3.5)**2)*1.0+np.exp(-np.abs(yy_-hz)/90)*0.30+np.exp(-np.abs(yy_-hz)/300)*0.12
side=np.exp(-((xx-vx)/(W*0.42))**2)
st=(rng.random((H,W))>0.9991)*(yy_<hz)*0.9
save('horizon',(g*0.9+glow*side+st))
