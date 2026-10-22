import test from 'node:test';
import assert from 'node:assert/strict';
import vm from 'node:vm';
import {readFileSync} from 'node:fs';
const source=readFileSync(new URL('./Sources/NotchPrototype/Resources/voice/session.js',import.meta.url),'utf8');
function harness({deferredCapture=false}={}) {
  const messages=[],peers=[],audios=[],timers=new Map(),sources=[],destinations=[];let calls=0,stops=0,resolveCapture;
  const track={enabled:true,stop(){stops++;}},stream={getTracks:()=>[track],getAudioTracks:()=>[track]};
  const capture=deferredCapture ? new Promise(resolve=>{resolveCapture=resolve}) : Promise.resolve(stream);
  class Peer {
    connectionState='new';localDescription=null;answers=[];
    constructor(){peers.push(this)}
    addTransceiver(){return {sender:{replaceTrack:async value=>{this.track=value}}}}
    createDataChannel(){return this.channel={readyState:'connecting'}}
    async createOffer(){return {type:'offer',sdp:'v=0\r\nOFFER'}}
    async setLocalDescription(value){this.localDescription=value}
    async setRemoteDescription(value){this.answers.push(value)}
    async getStats(){return new Map()}
    close(){this.connectionState='closed';this.onconnectionstatechange?.()}
  }
  class AudioContext {
    state='running';
    createAnalyser(){return {fftSize:256,getFloatTimeDomainData:array=>array.fill(.05)}}
    createMediaStreamSource(){return {connect(){}}}
    async decodeAudioData(bytes){return {bytes,duration:1}}
    createMediaStreamDestination(){
      const track={stopped:false,stop(){this.stopped=true}};
      const destination={stream:{getAudioTracks:()=>[track],getTracks:()=>[track]}};
      destinations.push(destination);return destination;
    }
    createBufferSource(){
      const source={buffer:null,started:false,stopped:false,connect(value){this.destination=value},start(){this.started=true},stop(){this.stopped=true}};
      sources.push(source);return source;
    }
    resume(){return Promise.resolve()}
    close(){return Promise.resolve()}
  }
  let nextTimer=0;
  const window={webkit:{messageHandlers:{kaiVoice:{postMessage:value=>{assert.ok(Object.values(value).every(field=>field!==undefined));messages.push(value)}}}},addEventListener(){}};
  const context=vm.createContext({window,RTCPeerConnection:Peer,AudioContext,Float32Array,MediaStream:class{},atob:value=>Buffer.from(value,'base64').toString('binary'),
    navigator:{mediaDevices:{getUserMedia:()=>{calls++;return capture}}},
    document:{body:{appendChild(){}},createElement(){const audio={style:{},play:()=>Promise.resolve(),pause(){},remove(){}};audios.push(audio);return audio}},
    setInterval:fn=>{timers.set(++nextTimer,fn);return nextTimer},setTimeout:fn=>{timers.set(++nextTimer,fn);return nextTimer},
    clearInterval:id=>timers.delete(id),clearTimeout:id=>timers.delete(id)});
  vm.runInContext(source,context);
  return {api:window.kaiVoice,messages,peers,audios,timers,track,stream,sources,destinations,get calls(){return calls},get stops(){return stops},resolveCapture};
}
test('startup holds microphone and output until explicitly activated',async()=>{
  const h=harness();assert.equal(h.messages[0].type,'ready');
  await h.api.start('one',true);
  assert.equal(h.calls,1);assert.equal(h.track.enabled,false);assert.equal(h.audios[0].muted,true);
  assert.equal(h.messages.find(m=>m.type==='offer').sdp,'v=0\r\nOFFER');
  const peer=h.peers[0];peer.connectionState='connected';peer.onconnectionstatechange();
  assert.equal(h.messages.filter(m=>m.type==='connected').length,0);
  peer.channel.readyState='open';peer.channel.onopen();
  assert.equal(h.messages.filter(m=>m.type==='connected').length,0);
  peer.channel.onmessage({data:JSON.stringify({type:'session.started',session:{id:'test'}})});
  peer.channel.onopen();
  assert.equal(h.messages.filter(m=>m.type==='connected').length,1);
  h.api.activate('one');assert.equal(h.track.enabled,true);assert.equal(h.audios[0].muted,false);
  h.api.stop();assert.equal(h.stops,1);assert.equal(peer.connectionState,'closed');assert.equal(h.timers.size,0);
});
test('capture resolving after cancellation is stopped and never offered',async()=>{
  const h=harness({deferredCapture:true}),pending=h.api.start('one',true);
  h.api.stop();h.resolveCapture(h.stream);await pending;
  assert.equal(h.stops,1);assert.equal(h.messages.filter(m=>m.type==='offer').length,0);
});
test('answer and activation are scoped to the current voice session',async()=>{
  const h=harness();await h.api.start('new',false);
  await h.api.answer('old','v=0 OLD');h.api.activate('old');
  assert.equal(h.peers[0].answers.length,0);assert.equal(h.audios[0].muted,true);
  await h.api.answer('new','v=0 ANSWER');assert.equal(h.peers[0].answers[0].sdp,'v=0 ANSWER');
  h.api.stop();await h.api.answer('new','LATE');assert.equal(h.peers[0].answers.length,1);
});
test('diagnostics never request microphone capture',async()=>{
  const h=harness();await h.api.start('test',false);
  assert.equal(h.calls,0);assert.ok(h.messages.some(m=>m.type==='offer'));assert.equal(h.audios[0].muted,true);
  h.api.stop();
});
test('peer and service failures retire media without any fallback',async()=>{
  const h=harness();await h.api.start('one',true);
  h.peers[0].channel.onmessage({data:JSON.stringify({type:'error',error:{message:'Not authorized'}})});
  assert.equal(h.messages.at(-1).type,'error');assert.equal(h.messages.at(-1).message,'Not authorized');
  assert.equal(h.calls,1);assert.equal(h.stops,1);assert.equal(h.timers.size,0);
});
test('a disconnected old peer cannot terminate a new session',async()=>{
  const h=harness();await h.api.start('one',false);const old=h.peers[0];
  old.connectionState='disconnected';old.onconnectionstatechange();const callbacks=[...h.timers.values()];
  await h.api.start('two',false);
  for(const callback of callbacks)await callback();
  assert.equal(h.messages.filter(m=>m.type==='error').length,0);
  assert.equal(h.peers[1].connectionState,'new');h.api.stop();
});
test('held first prompt goes to native WebRTC before enabling live microphone',async()=>{
  const h=harness(),wav=Buffer.from('recorded initial prompt');
  await h.api.start('held',true,wav.toString('base64'));
  assert.equal(h.track.enabled,false);assert.equal(h.sources.length,0);
  await h.api.activate('held');
  assert.equal(h.track.enabled,false);assert.equal(h.sources.length,1);
  const source=h.sources[0],destination=h.destinations[0];
  assert.equal(source.started,true);assert.equal(source.destination,destination);
  assert.deepEqual(Buffer.from(source.buffer.bytes),wav);
  assert.equal(h.peers[0].track,destination.stream.getAudioTracks()[0]);
  await source.onended();
  assert.equal(h.peers[0].track,h.track);assert.equal(h.track.enabled,true);
  assert.equal(destination.stream.getAudioTracks()[0].stopped,true);
  assert.equal(h.messages.filter(m=>m.type==='promptSent').length,1);
  await h.api.activate('held');assert.equal(h.sources.length,1);
  h.api.stop();
});
test('cancelling initial audio prevents a late callback from enabling any microphone',async()=>{
  const h=harness();await h.api.start('held',true,'YXVkaW8=');await h.api.activate('held');
  const source=h.sources[0],ended=source.onended;
  h.api.stop();await ended();
  assert.equal(source.stopped,true);assert.equal(h.track.enabled,false);assert.equal(h.stops,1);
  assert.equal(h.destinations[0].stream.getAudioTracks()[0].stopped,true);
  assert.equal(h.messages.filter(m=>m.type==='promptSent').length,0);assert.equal(h.timers.size,0);
});
test('speech and output events update compact activity without exposing message contents',async()=>{
  const h=harness();await h.api.start('one',true);
  for(const type of ['input_audio_buffer.speech_started','input_audio_buffer.speech_stopped','output_audio_buffer.started','output_audio_buffer.stopped']) {
    h.peers[0].channel.onmessage({data:JSON.stringify({type,text:'private transcript'})});
  }
  assert.deepEqual(h.messages.filter(m=>m.type==='activity').map(m=>m.activity),['listening','thinking','speaking','listening']);
  assert.equal(JSON.stringify(h.messages).includes('private transcript'),false);
  h.api.stop();
});
test('held-prompt diagnostic transmits only its generated fixture and never unmutes real input or output',async()=>{
  const h=harness();await h.api.start('probe',true,'YXVkaW8=',true);await h.api.activate('probe');
  assert.equal(h.track.enabled,false);assert.equal(h.audios[0].muted,true);
  await h.sources[0].onended();
  assert.equal(h.track.enabled,false);assert.equal(h.audios[0].muted,true);h.api.stop();
});
