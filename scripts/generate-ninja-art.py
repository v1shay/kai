#!/usr/bin/env python3
"""Deterministic pixel-art Ninja loops. No image service or runtime generator.
GIF89a, 48x56, 12 frames, transparent palette and bounded LZW literal blocks.
"""
from pathlib import Path
import math,struct
ROOT=Path(__file__).resolve().parents[1]/'integrations/Code/assets'
COLORS={'fire':0xef6758,'lightning':0x619dff,'earth':0xa5acba,'ice':0xc2edf4,'energy':0x74d6a0,'water':0x60ccd7}
def encode(pixels):
    # Reset before the 7-bit LZW dictionary grows. Deliberately simple and portable.
    codes=[]
    for start in range(0,len(pixels),40):codes.extend([64]+pixels[start:start+40])
    codes.append(65);out=bytearray();bits=0;count=0
    for code in codes:
        bits|=code<<count;count+=7
        while count>=8:out.append(bits&255);bits>>=8;count-=8
    if count:out.append(bits&255)
    return b'\x06'+b''.join(bytes([len(out[i:i+255])])+out[i:i+255] for i in range(0,len(out),255))+b'\0'
def make(color):
    rgb=((color>>16)&255,(color>>8)&255,color&255)
    palette=[(0,0,0),(18,24,37),(242,231,200),(12,17,26)]+[tuple(int(c*(i/59)) for c in rgb) for i in range(60)]
    data=bytearray(b'GIF89a'+struct.pack('<HHBBB',48,56,0xf5,0,0)+bytes(c for p in palette for c in p))
    data+=b'!\xff\x0bNETSCAPE2.0\x03\x01\0\0\0'
    for frame in range(12):
        pixels=[0]*(48*56);phase=frame*math.tau/12;bob=round(math.sin(phase)*.8)
        def dot(x,y,c):
            if 0<=x<48 and 0<=y<56:pixels[y*48+x]=c
        def rect(x,y,w,h,c):
            for yy in range(y,y+h):
                for xx in range(x,x+w):dot(xx,yy,c)
        # Sparse ordered dither aura: every dot remains aligned to the pixel grid.
        for y in range(4,54,2):
            for x in range(2,47,2):
                distance=math.hypot((x-24)/1.1,y-30);wave=(math.sin(distance*.48-phase)+1)/2
                if 12<distance<25 and (x*17+y*29)%13 < 3:dot(x,y,4+int(wave*19))
        y=11+bob
        rect(17,y,14,3,46);rect(14,y+3,20,16,54);rect(16,y+19,16,3,39)
        rect(14,y+4,3,12,33);rect(30,y+4,4,12,44)
        rect(16,y+8,16,7,2);rect(18,y+10,4,2,3);rect(26,y+10,4,2,3)
        rect(17,y+15,14,3,58);rect(18,y+18,12,2,46)
        # Scarf tails catch the breeze; each style keeps its exact identity color.
        tail=round(math.sin(phase)*2);rect(32,y+15,7,3,54);rect(37,y+17,3,7+tail,39)
        rect(18,y+23,12,11,48);rect(20,y+24,8,2,57);rect(16,y+27,3,7,34);rect(30,y+27,3,7,34)
        rect(18,y+34,5,6,25);rect(25,y+34,5,6,30);rect(17,y+39,6,2,1);rect(25,y+39,7,2,1)
        for row in range(26,33,2):dot(20+(row%3),y+row,60)
        # Brief eye glint, not a full blink that makes the face disappear.
        if frame==8:dot(18,y+10,2);dot(26,y+10,2)
        data+=b'!\xf9\x04\x09'+struct.pack('<H',8)+b'\0\0'+b','+struct.pack('<HHHHB',0,0,48,56,0)+encode(pixels)
    return data+b';'
if __name__=='__main__':
    for name,color in COLORS.items():(ROOT/f'ninja-{name}.gif').write_bytes(make(color))
