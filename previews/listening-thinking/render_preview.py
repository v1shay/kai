#!/usr/bin/env python3
"""Render a lightweight looping review GIF for the interactive SVG motion."""
import math, subprocess

W,H,FPS,SECONDS=720,420,30,12
MIN,MAXH,CY=32,178,210
BASE=bytes((8,10,14))*(W*H)

def clamp(v,a=0,b=1): return max(a,min(b,v))
def smooth(t): return t*t*(3-2*t)
def speech(t): return .10+.78*max(0,math.sin(t*7.4))*max(0,math.sin(t*2.05+.5))+.11*math.sin(t*13.1)**2
def listening(t):
    a=speech(t); return [MIN+(MAXH-MIN)*clamp(a*(.58+i*.045+.28*math.sin(t*(10.7+i*.73)+i*1.37)**2)) for i in range(4)]
def thinking(t):
    dots=[]
    for i,base in enumerate((24,29,34,39)):
        pulse=smooth(.5+.5*math.sin(t*4.5-i*.9)); dots.append((base+2.2*pulse,-4*pulse,.42+.58*pulse))
    return dots
def rounded_distance(px,py,cx,cy,w,h,r):
    qx=abs(px-cx)-(w/2-r); qy=abs(py-cy)-(h/2-r)
    return math.hypot(max(qx,0),max(qy,0))+min(max(qx,qy),0)-r
def draw(frame,cx,height,width=30,offset=0,opacity=1):
    cy=CY+offset; top=cy-height/2; bottom=cy+height/2
    for y in range(max(0,int(top-18)),min(H,int(bottom+19))):
        for x in range(max(0,int(cx-width/2-18)),min(W,int(cx+width/2+19))):
            d=rounded_distance(x+.5,y+.5,cx,cy,width,height,width/2)
            if d<=0: alpha=opacity; p=clamp((y-top)/height); color=(round(250-112*p),round(250-108*p),round(250-103*p))
            elif d<18: alpha=opacity*.30*math.exp(-(d*d)/42); color=(205,220,240)
            else: continue
            k=(y*W+x)*3
            frame[k:k+3]=bytes(round(frame[k+i]*(1-alpha)+color[i]*alpha) for i in range(3))

cmd=['ffmpeg','-y','-v','error','-f','rawvideo','-pix_fmt','rgb24','-s',f'{W}x{H}','-r',str(FPS),'-i','-',
     '-filter_complex','[0:v]split[a][b];[a]palettegen=stats_mode=diff[p];[b][p]paletteuse=dither=bayer:bayer_scale=3',
     '-loop','0','preview.gif']
process=subprocess.Popen(cmd,stdin=subprocess.PIPE)
start_transition=4.2; blend=.90; initial=listening(start_transition)
for n in range(FPS*SECONDS):
    t=n/FPS
    if t<start_transition: shapes=[(h,30,0,1) for h in listening(t)]
    elif t<start_transition+blend:
        e=smooth((t-start_transition)/blend); target=thinking(t); shapes=[]
        for i,(size,y,alpha) in enumerate(target): shapes.append((initial[i]+(size-initial[i])*e,30+(size-30)*e,y*e,1+(alpha-1)*e))
    else: shapes=[(size,size,y,alpha) for size,y,alpha in thinking(t)]
    frame=bytearray(BASE)
    for x,(h,w,y,a) in zip((270,330,390,450),shapes): draw(frame,x,h,w,y,a)
    process.stdin.write(frame)
process.stdin.close(); raise SystemExit(process.wait())
