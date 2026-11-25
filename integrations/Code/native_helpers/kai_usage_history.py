#!/usr/bin/env python3
"""Local usage history index. Reads numerical metadata only; never sends transcripts.
Formats researched against T3 Code's MIT usage readers, independent implementation.
"""
import argparse, datetime as dt, hashlib, json, os, pathlib, sqlite3, time, urllib.request, re, math

VERSION = 6
MAX_LINE = 4 * 1024 * 1024

def timestamp(value):
    if isinstance(value, (int, float)):
        return (value / 1000 if value > 1e11 else value) if math.isfinite(value) else 0
    try:
        return dt.datetime.fromisoformat(value.replace('Z', '+00:00')).timestamp()
    except (ValueError, TypeError, AttributeError):
        return 0

def number(v):
    if isinstance(v, bool) or not isinstance(v, (int, float)) or not math.isfinite(v) or v < 0:
        raise ValueError('invalid counter')
    return int(v)

def codex_usage(u):
    i, c, o = (number(u.get(k, 0)) for k in ('input_tokens', 'cached_input_tokens', 'output_tokens'))
    w = number(u.get('cache_write_input_tokens', 0))
    if c + w > i or number(u.get('reasoning_output_tokens', 0)) > o or number(u.get('total_tokens',i+o)) < i+o:
        raise ValueError('inconsistent counter')
    return [i-c-w, c, w, o]

def parse(provider, record, state, filekey, line):
    p = record.get('payload') or {}
    at = timestamp(record.get('timestamp'))
    if provider == 'Codex':
        if record.get('type') == 'session_meta':
            if state.get('saw_meta'):return None
            state['saw_meta']=True;state['session'] = p.get('id',p.get('session_id',filekey))
            source=p.get('source') or {};subagent=source.get('subagent') or {} if isinstance(source,dict) else {};spawn=subagent.get('thread_spawn') or {} if isinstance(subagent,dict) else {}
            state['fork']=bool(p.get('forked_from_id') or (isinstance(spawn,dict) and spawn.get('parent_thread_id')));state['fork_anchor']=at
        if record.get('type') == 'turn_context':
            state['model'] = p.get('model', state.get('model', 'Unknown'))
            state['ledger'] = False
        if p.get('type') == 'thread_settings_applied':
            state['speed'] = (p.get('thread_settings') or {}).get('service_tier') or 'standard'
        if record.get('type') == 'token_usage_record':
            u = codex_usage(p.get('usage') or {})
            response = p.get('response_id')
            if not response: return None
            state['ledger'] = True
            state['total'] = [a+b for a,b in zip(state.get('total', [0]*4), u)]
            key = 'codex:' + str(response)
        elif p.get('type') == 'token_count':
            info = p.get('info') or {}
            total = info.get('total_token_usage')
            current = codex_usage(total) if total else None
            old = state.get('total');age=state.get('baseline_at',0)
            if current:state['total']=current;state['baseline_at']=max(at,age)
            if current == old or state.get('ledger'):return None
            last=info.get('last_token_usage')
            if not isinstance(last,dict):raise ValueError('missing request counters')
            u=codex_usage(last)
            if last.get('total_tokens',last.get('input_tokens',0)+last.get('output_tokens',0)) != last.get('input_tokens',0)+last.get('output_tokens',0):raise ValueError('context marker is not a request')
            if current:
                restarted=current==u;sequence=[at,u]
                if restarted and state.get('sequence')==sequence:
                    state['total']=old;return None
                delta=[a-b for a,b in zip(current,old)] if old else None
                rewound=old and any(a<b for a,b in zip(current,old)) and at>age
                prefix_valid=all(a>=b for a,b in zip(current,u))
                if not restarted and not ((old is None or rewound) and prefix_valid) and delta!=u:
                    if at<=age:state['total']=old;state['baseline_at']=age
                    raise ValueError('snapshot does not identify the unseen request prefix')
                if restarted:state['sequence']=sequence
            else:
                if state.get('last_signature')==u:return None
                state['total']=[a+b for a,b in zip(old or [0]*4,u)];state['baseline_at']=max(at,age)
            state['last_signature']=u
            # Bill the reported request, never an inherited cumulative prefix.
            key = 'codex:' + str(state.get('session',filekey)) + ':' + json.dumps(current or [at, u])
        else: return None
        if state.get('fork'):
            # T3's MIT reader identifies the tightly restamped copied burst;
            # a later real model turn ends suppression for this rollout.
            copied=at-state.get('fork_anchor',at)<1
            state['fork_anchor']=at
            if copied:return None
            state['fork']=False
        model = state.get('model', 'Unknown')
        session = state.get('session', filekey)
        speed = state.get('speed', 'standard')
        cost = None
    elif provider == 'Claude':
        m = record.get('message') or {}
        if m.get('role') != 'assistant' or not isinstance(m.get('usage'), dict): return None
        raw = m['usage']
        u = [number(raw.get(k,0)) for k in ('input_tokens','cache_read_input_tokens','cache_creation_input_tokens','output_tokens')]
        model, session, speed = m.get('model','Unknown'), record.get('sessionId',filekey), 'standard'
        key = 'claude:' + str(m.get('id') or f'{filekey}:{line}') + ':' + str(record.get('requestId',''))
        cost = None
    elif provider == 'Grok Build':
        params=record.get('params') or {}; update=params.get('update') or {}
        if update.get('sessionUpdate')!='turn_completed':return None
        raw=update.get('usage') or {};i=number(raw.get('inputTokens',0));c=number(raw.get('cachedReadTokens',0));w=number(raw.get('cacheCreationTokens',0));o=number(raw.get('outputTokens',0))
        u=[max(0,i-c-w),c,w,o];model=next(iter(raw.get('modelUsage') or {}),'grok');session=params.get('sessionId',filekey);speed='standard'
        at=timestamp((params.get('_meta') or {}).get('agentTimestampMs') or record.get('timestamp'))
        key='grok:'+str(session)+':'+str(update.get('prompt_id') or line)
        cost=raw.get('costUsdTicks');cost=cost/1e10 if isinstance(cost,(float,int)) and cost>=0 else None
    elif provider == 'OpenCode':
        if record.get('role')!='assistant' or not isinstance(record.get('tokens'),dict):return None
        raw=record['tokens'];cache=raw.get('cache') or {};u=[number(raw.get('input',0)),number(cache.get('read',0)),number(cache.get('write',0)),number(raw.get('output',0))+number(raw.get('reasoning',0))]
        ref=record.get('model') or {};model=ref.get('id',ref.get('modelID',record.get('modelID','Unknown'))) if isinstance(ref,dict) else str(ref)
        at=timestamp((record.get('time') or {}).get('created'));session=record.get('sessionID',filekey);speed='standard';key='opencode:'+str(record.get('id') or filekey)
        cost=record.get('cost');cost=cost if isinstance(cost,(int,float)) and cost>0 else 0 if ((ref.get('providerID') or record.get('providerID')) if isinstance(ref,dict) else record.get('providerID')) in ('opencode','opencode-go') and model.endswith('-free') else None
    else: return None
    if not u or not sum(u) or not at: return None
    return (key, provider, str(session), str(model), at, *u, str(speed), cost)

def roots():
    home = pathlib.Path.home()
    c = pathlib.Path(os.environ.get('CODEX_HOME', home/'.codex'))
    a = pathlib.Path(os.environ.get('CLAUDE_CONFIG_DIR', home/'.claude'))
    return [('Codex', c/'sessions'), ('Codex', c/'archived_sessions'), ('Claude', a/'projects'),('Grok Build', pathlib.Path(os.environ.get('GROK_HOME',home/'.grok'))/'sessions')]

def index(db, scan):
    db.execute('create table if not exists files(path text primary key, size integer, stamp integer, inode integer, offset integer, state text, bad integer)')
    db.execute('create table if not exists records(key text primary key, provider text, session text, model text, at real, input integer, cache integer, write integer, output integer, speed text, cost real)')
    db.execute('create index if not exists time_index on records(at)')
    if not scan: return
    cutoff = time.time()-93*86400
    for provider, root in roots():
        if not root.is_dir(): continue
        for path in root.rglob('*.jsonl'):
            try:
                st = path.stat()
                if st.st_mtime < cutoff: continue
                name = str(path.resolve())
                old = db.execute('select size,stamp,inode,offset,state,bad from files where path=?',(name,)).fetchone()
                if old and old[:3] == (st.st_size,st.st_mtime_ns,st.st_ino): continue
                append = old and old[2] == st.st_ino and st.st_size > old[0]
                offset, state, bad = (old[3],json.loads(old[4]),old[5]) if append else (0,{},0)
                with path.open('rb') as f:
                    f.seek(offset)
                    while True:
                        start = f.tell()
                        raw = f.readline(MAX_LINE+1)
                        if not raw: break
                        if len(raw) > MAX_LINE:
                            while raw and not raw.endswith(b'\n'): raw = f.readline(MAX_LINE+1)
                            bad += 1
                            offset = f.tell()
                            continue
                        if not raw.endswith(b'\n'): break  # retry partially written records next refresh
                        offset = f.tell()
                        try:
                            record = json.loads(raw)
                            row = parse(provider,record,state,name,start)
                            if row:
                                db.execute('insert into records values(?,?,?,?,?,?,?,?,?,?,?) on conflict(key) do update set input=excluded.input,cache=excluded.cache,write=excluded.write,output=excluded.output where excluded.input+excluded.cache+excluded.write+excluded.output >= records.input+records.cache+records.write+records.output',row)
                        except (ValueError,TypeError,AttributeError,KeyError): bad += 1
                db.execute('insert or replace into files values(?,?,?,?,?,?,?)',(name,st.st_size,st.st_mtime_ns,st.st_ino,offset,json.dumps(state),bad))
                db.commit()
            except (OSError,sqlite3.Error): continue
    db.execute('delete from records where at < ?',(cutoff,))
    db.commit()

def opencode_index(db):
    root=pathlib.Path(os.environ.get('OPENCODE_DATA_DIR',pathlib.Path.home()/'.local/share/opencode'))
    candidates=list(root.glob('*.db'))+list(root.glob('*.sqlite'))
    for path in candidates:
        try:
            marker=str(path.resolve());st=path.stat();wal=pathlib.Path(str(path)+'-wal');stamp=max(st.st_mtime_ns,wal.stat().st_mtime_ns if wal.exists() else 0)
            old=db.execute('select stamp from files where path=?',(marker,)).fetchone()
            if old and old[0]==stamp:continue
            with sqlite3.connect(path.resolve().as_uri()+'?mode=ro',uri=True,timeout=1) as source:
                columns={r[1] for r in source.execute('pragma table_info(message)')}
                if 'data' not in columns:continue
                query='select data'+(',id' if 'id' in columns else ",''")+(',session_id' if 'session_id' in columns else ",''")+' from message'
                for raw,id,sid in source.execute(query):
                    try:
                        message=json.loads(raw);message.setdefault('id',id);message.setdefault('sessionID',sid)
                        row=parse('OpenCode',message,{},marker,0)
                        if row:db.execute('insert or replace into records values(?,?,?,?,?,?,?,?,?,?,?)',row)
                    except (ValueError,TypeError,AttributeError):continue
            db.execute('insert or replace into files values(?,?,?,?,?,?,?)',(marker,st.st_size,stamp,st.st_ino,0,'{}',0));db.commit()
        except (OSError,sqlite3.Error):continue
    for path in (root/'storage/message').glob('**/*.json'):
        try:
            st=path.stat();marker=str(path.resolve());old=db.execute('select stamp from files where path=?',(marker,)).fetchone()
            if old and old[0]==st.st_mtime_ns:continue
            if st.st_size>MAX_LINE:continue
            row=parse('OpenCode',json.loads(path.read_text()),{},marker,0)
            if row:db.execute('insert or replace into records values(?,?,?,?,?,?,?,?,?,?,?)',row)
            db.execute('insert or replace into files values(?,?,?,?,?,?,?)',(marker,st.st_size,st.st_mtime_ns,st.st_ino,0,'{}',0))
        except (OSError,ValueError,TypeError,AttributeError):continue
    db.commit()

# Antigravity metadata wire schema/model IDs adapted from T3 Code (MIT).
# See LICENSES/T3-Code-MIT.txt and docs/research/2026-11-13-usage-and-session-owners.md.
AG_MODELS={
  246: "gemini-2.5-pro",
  312: "gemini-2.5-flash",
  313: "gemini-2.5-flash-thinking",
  329: "gemini-2.5-flash-thinking",
  330: "gemini-2.5-flash-lite",
  281: "claude-sonnet-4",
  282: "claude-sonnet-4",
  290: "claude-opus-4",
  291: "claude-opus-4",
  333: "claude-sonnet-4-5",
  334: "claude-sonnet-4-5",
  340: "claude-haiku-4-5",
  341: "claude-haiku-4-5",
  342: "gpt-oss-120b-medium",
  1026: "claude-opus-4-6",
  1035: "claude-sonnet-4-6",
  1016: "gemini-3.1-pro",
  1036: "gemini-3.1-pro",
  1037: "gemini-3.1-pro",
  1018: "gemini-3-flash-preview",
  1084: "gemini-3-flash-preview",
  1047: "gemini-3-flash-preview",
  1020: "gemini-3.5-flash-medium",
  1021: "gemini-3.1-flash-image",
  1050: "gemini-3.1-flash-lite",
  1071: "gemini-3.6-flash-high",
  1072: "gemini-3.6-flash-medium",
  1073: "gemini-3.6-flash-low",
  1132: "gemini-3.5-flash-high",
  1133: "gemini-3.5-flash-high",
  1187: "gemini-3.5-flash-extra-low",
  1196: "gemini-3.6-flash",
  1266: "gemini-3.6-flash",
  1298: "gemini-3.7-flash-high",
  1299: "gemini-3.7-flash-medium",
  1300: "gemini-3.7-flash-low",
  1301: "gemini-3.7-flash-tiered",
  1318: "gemini-3.8-flash-high",
  1319: "gemini-3.8-flash-medium",
  1320: "gemini-3.8-flash-low",
  1322: "gemini-3.8-flash-tiered",
  1402: "claude-opus-5-5",
  1417: "gemini-4-argon",
  1418: "gemini-4-argon",
  1419: "gemini-4-argon",}
def pb(raw):
    result={};offset=0
    def varint():
        nonlocal offset
        value=0
        for shift in range(0,70,7):
            if offset>=len(raw):raise ValueError('truncated protobuf')
            byte=raw[offset];offset+=1;value|=(byte&127)<<shift
            if byte<128:return value
        raise ValueError('invalid varint')
    while offset<len(raw):
        tag=varint();field,wire=tag>>3,tag&7
        if field==0:raise ValueError('invalid field')
        if wire==0:value=varint()
        elif wire in (1,2,5):
            length=varint() if wire==2 else 8 if wire==1 else 4
            if length>len(raw)-offset:raise ValueError('truncated field')
            value=raw[offset:offset+length];offset+=length
            if wire!=2:continue
        else:raise ValueError('unsupported wire type')
        result.setdefault(field,[]).append(value)
    return result

def pbnum(fields,key):
    v=fields.get(key,[0])[0];return v if isinstance(v,int) else 0

def pbbytes(fields,key):
    v=fields.get(key,[b''])[0];return v if isinstance(v,bytes) else b''

def pbnested(fields,key):return pb(pbbytes(fields,key))
def pbtext(fields,key):return pbbytes(fields,key).decode('utf-8').strip()
def pbtime(fields):return pbnum(fields,1)+pbnum(fields,2)/1e9 if pbnum(fields,1)>0 else 0

def ag_metadata(raw,step):
    root=pb(raw);data=root if step else pbnested(root,1);model=pbnested(data,24) if step else data
    usage=pbbytes(data,9 if step else 4);usages=[pb(usage)] if usage else []
    for retry in data.get(28 if step else 17,[]):
        item=pbbytes(pb(retry),2)
        if item:usages.append(pb(item))
    names=[]
    for key in ([12,8] if step else [19,21]):
        name=pbtext(model,key).lower();name=re.sub(r'\s*\(([^)]*)\)\s*$',r'-\1',name);name=re.sub(r'\s+','-',name)
        if name.startswith('claude-'):name=re.sub(r'^claude-(\d+(?:\.\d+)?)-(sonnet|opus|haiku)',r'claude-\2-\1',name).replace('.','-')
        if name:names.append(name)
    return dict(model=next((n for n in names if re.match(r'^(gemini-\d|claude-(sonnet|opus|haiku)-\d|gpt-)',n)),next(iter(names),'')),model_id=pbnum(model,1 if step else 3) or (pbnum(usages[0],1) if usages else 0),at=(pbtime(pbnested(data,8)) or pbtime(pbnested(data,1))) if step else pbtime(pbnested(pbnested(data,9),4)),usages=usages)

def antigravity_index(db):
    home=pathlib.Path.home();configured=os.environ.get('ANTIGRAVITY_DATA_DIR')
    roots=[pathlib.Path(p.strip()).expanduser() for p in configured.split(',')] if configured else [home/'.gemini'/n for n in ('antigravity','antigravity-cli','antigravity-ide','antigravity-backup')]+[home/'.config/antigravity']
    candidates=[];learned={};bad=0
    for path in sorted({p.resolve() for root in roots if root.is_dir() for p in root.rglob('*.db')}):
        try:
            st=path.stat();wal=pathlib.Path(str(path)+'-wal');stamp=max(st.st_mtime_ns,wal.stat().st_mtime_ns if wal.exists() else 0)
            old=db.execute('select stamp,state from files where path=?',(str(path),)).fetchone()
            if old and old[0]==stamp:
                cached=json.loads(old[1]);candidates.extend(cached.get('candidates',[]));learned.update(cached.get('models',{}));continue
            local=[];names={}
            with sqlite3.connect(path.as_uri()+'?mode=ro',uri=True,timeout=1) as source:
                tables={r[0] for r in source.execute("select name from sqlite_master where type='table'")}
                trajectory=0
                if 'trajectory_metadata_blob' in tables:
                    for(raw,)in source.execute('select data from trajectory_metadata_blob where length(data)<4194304'):
                        trajectory=trajectory or pbtime(pbnested(pb(raw),2))
                entries=[]
                for table,column,step in [('gen_metadata','data',False),('steps','metadata',True)]:
                    if table not in tables:continue
                    for idx,raw in source.execute(f'select idx,{column} from {table} where {column} is not null and length({column})<4194304 order by idx'):
                        try:entries.append((idx,step,ag_metadata(raw,step)))
                        except (ValueError,TypeError,UnicodeDecodeError):bad+=1
                for _,_,entry in entries:
                    if entry['model'] and entry['model_id']:
                        key=str(entry['model_id']);prev=names.get(key,entry['model']);names[key]=entry['model'] if prev==entry['model'] else ''
                gens={idx:entry for idx,step,entry in entries if not step}
                for idx,step,entry in entries:
                    for k,usage in enumerate(entry['usages']):
                        model_id=pbnum(usage,1) or entry['model_id'];model=entry['model'] if not model_id or model_id==entry['model_id'] else ''
                        if not model and idx in gens and gens[idx]['model_id']==model_id:model=gens[idx]['model']
                        u=[pbnum(usage,2),pbnum(usage,5),pbnum(usage,4),max(pbnum(usage,3),pbnum(usage,9)+pbnum(usage,10))]
                        if not sum(u):continue
                        keys=['antigravity:'+str(n)+':'+pbtext(usage,n) for n in (11,12,7) if pbtext(usage,n)]
                        key=keys[0] if keys else f'antigravity:{path.stem}:{step}:{idx}:{k}'
                        local.append(dict(key=key,aliases=keys,session=path.stem,model=model,model_id=model_id,at=entry['at'] or trajectory or st.st_mtime,u=u))
            cached=dict(candidates=local,models=names);db.execute('insert or replace into files values(?,?,?,?,?,?,?)',(str(path),st.st_size,stamp,st.st_ino,0,json.dumps(cached),bad));db.commit();candidates.extend(local);learned.update(names)
        except (OSError,sqlite3.Error,ValueError,TypeError,UnicodeDecodeError):continue
    # Union by every recorded request identity, including copied databases/retries.
    groups=[];identities={};parents=[]
    def find(i):
        while parents[i]!=i:parents[i]=parents[parents[i]];i=parents[i]
        return i
    for candidate in candidates:
        n=len(groups);groups.append(candidate);parents.append(n)
        for alias in candidate['aliases']:
            if alias in identities:
                a,b=find(n),find(identities[alias])
                if a!=b:
                    x,y=groups[a],groups[b];x['u']=[max(v,w) for v,w in zip(x['u'],y['u'])];x['model']=x['model'] or y['model'];parents[b]=a
            identities[alias]=n
    db.execute("delete from records where provider='Antigravity'")
    for n,candidate in enumerate(groups):
        if find(n)!=n:continue
        model=candidate['model'] or learned.get(str(candidate['model_id'])) or AG_MODELS.get(candidate['model_id']) or f"antigravity-model-{candidate['model_id']}"
        row=(candidate['key'],'Antigravity',candidate['session'],model,candidate['at'],*candidate['u'],'standard',None)
        db.execute('insert or replace into records values(?,?,?,?,?,?,?,?,?,?,?)',row)
    db.commit()

def pricing(cache, refresh):
    path = cache/'prices.json'
    try:
        if refresh and (not path.exists() or time.time()-path.stat().st_mtime > 86400):
            req = urllib.request.Request('https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json',headers={'User-Agent':'Kai-Usage/1'})
            with urllib.request.urlopen(req,timeout=12) as r: raw = r.read(8*1024*1024)
            json.loads(raw)
            tmp = path.with_suffix('.next'); tmp.write_bytes(raw); tmp.replace(path)
        return json.loads(path.read_text())
    except (OSError,ValueError): pass
    return {}

def local_rates(rates):
    # T3's cache is a public price document, not account credentials.
    try:
        document = json.loads((pathlib.Path.home()/'.t3/userdata/usage-model-rates.json').read_text())['document']
        return dict(document, **rates)
    except (OSError,ValueError,KeyError,TypeError): return rates

def optional_t3_index(db):
    # Import numerical metadata for compressed/opaque provider stores. Directly
    # indexed Codex/Claude files remain owned by our incremental reader.
    path = pathlib.Path.home()/'.t3/userdata/usage-scan-cache-v15.json'
    try:
        if path.stat().st_size > 64*1024*1024: return
        data=json.loads(path.read_text())
        if data.get('version') != 15: return
        models,sessions=data['models'],data['sessions']
        for name,entry in data['files'].items():
            prov=entry.get('p')
            if not name.endswith('.zst'): continue
            provider={'codex':'Codex','claude':'Claude','grok':'Grok Build','antigravity':'Antigravity','opencode':'OpenCode'}.get(prov)
            if not provider: continue
            for r in entry.get('r',[])+entry.get('t',[]):
                if len(r)<11 or r[0]/1000 < time.time()-93*86400:continue
                counters=[number(v) for v in r[3:7]]
                key='t3:'+prov+':'+str(r[8] or [sessions[r[2]],r[0],counters])
                if prov=='codex' and isinstance(r[8],str):
                    try:
                        identity=json.loads(r[8])
                        if isinstance(identity,list) and len(identity)>=3 and identity[0]=='codex':key='codex:'+str(identity[-1])
                    except ValueError:pass
                row=(key,provider,sessions[r[2]],models[r[1]],r[0]/1000,*counters,['standard','fast','ultrafast'][r[10]],r[9])
                db.execute('insert or replace into records values(?,?,?,?,?,?,?,?,?,?,?)',row)
        db.commit()
    except (OSError,ValueError,TypeError,KeyError,IndexError,sqlite3.Error): pass


def report(db, rates, days, provider):
    now = time.time(); step = 3600 if days == 1 else 86400
    if days == 1:
        start=(int(now)//step-23)*step;bucket_keys=[start+i*step for i in range(24)]
    else:
        dates=[dt.date.today()-dt.timedelta(days=days-1-i) for i in range(days)]
        bucket_keys=[int(time.mktime((d.year,d.month,d.day,0,0,0,0,0,-1))) for d in dates];start=bucket_keys[0]
    rows = db.execute('select key,provider,session,model,at,input,cache,write,output,speed,cost from records where at >= ? order by at',(start,))
    out = dict(cost=0.,tokens=0,input=0,cache=0,write=0,output=0,savings=0.,unpriced=0,sessions=0, providers=[],models=[],series=[],types=[0.,0.,0.],speeds={},premium=0.,warnings=[])
    groups = {}; models = {}; sessions = set(); buckets = {at: dict(at=at,cost=0.,tokens=0) for at in bucket_keys}
    tier_cache={};resolved_rates={}
    def resolve_rate(prov,model,speed,prompt):
        identity=(prov,model)
        if identity not in tier_cache:
            rate_model=re.sub(r'-(?:extra-low|low|medium|high|tiered|control|thinking|agent)$','',model) if prov=='Antigravity' else model
            raw=rates.get(model) or rates.get(rate_model) or rates.get(model.removeprefix('anthropic/'))
            tiers=[]
            if raw:
                for field in raw:
                    match=re.fullmatch(r'input_cost_per_token_above_(\d+)([km]?)_tokens(?:_priority)?',field)
                    if match:tiers.append((int(match[1])*{'':1,'k':1000,'m':1000000}[match[2]],'_above_'+match[1]+match[2]+'_tokens'))
            tier_cache[identity]=(raw,sorted(tiers,reverse=True))
        raw,tiers=tier_cache[identity]
        if not raw or raw.get('input_cost_per_token') is None or raw.get('output_cost_per_token') is None:return None,None
        suffix=next((suffix for threshold,suffix in tiers if prompt>threshold),'')
        key=(identity,speed,suffix)
        if key not in resolved_rates:
            fast='_priority' if speed in ('fast','priority') else ''
            effective={};standard={}
            for name in ('input_cost_per_token','output_cost_per_token','cache_read_input_token_cost','cache_creation_input_token_cost'):
                standard[name]=raw.get(name+suffix,raw.get(name))
                effective[name]=raw.get(name+suffix+fast,standard[name])
            resolved_rates[key]=(effective,standard)
        return resolved_rates[key]
    for key,prov,sid,model,at,i,c,w,o,speed,reported in rows:
        if provider != 'All providers' and provider != prov: continue
        rate,standard=resolve_rate(prov,model,speed,i+c+w)
        costs=None
        if rate and rate.get('input_cost_per_token') is not None and rate.get('output_cost_per_token') is not None and (not c or rate.get('cache_read_input_token_cost') is not None) and (not w or rate.get('cache_creation_input_token_cost') is not None):
            costs = [i*rate['input_cost_per_token']+w*(rate.get('cache_creation_input_token_cost') or 0), c*(rate.get('cache_read_input_token_cost') or 0), o*rate['output_cost_per_token']]
            out['savings'] += c*max(0,rate['input_cost_per_token']-(rate.get('cache_read_input_token_cost') or 0))
        cost = reported if reported is not None else sum(costs) if costs is not None else 0
        if costs is None and reported is None: out['unpriced'] += 1
        tokens = i+c+w+o
        for k,v in [('cost',cost),('tokens',tokens),('input',i),('cache',c),('write',w),('output',o)]: out[k] += v
        if costs:
            out['types'] = [a+b for a,b in zip(out['types'],costs)]
        category='Fast' if speed in ('fast','priority') else 'Ultrafast' if speed=='ultrafast' else 'Standard'
        out['speeds'][category] = out['speeds'].get(category,0)+cost
        if costs and category=='Fast':
            baseline=i*standard['input_cost_per_token']+w*(standard.get('cache_creation_input_token_cost') or 0)+c*(standard.get('cache_read_input_token_cost') or 0)+o*standard['output_cost_per_token']
            out['premium']+=max(0,sum(costs)-baseline)
        sessions.add((prov,sid))
        if days==1:bucket=int(at)//step*step
        else:
            d=dt.datetime.fromtimestamp(at);bucket=int(time.mktime((d.year,d.month,d.day,0,0,0,0,0,-1)))
        if bucket in buckets:
            buckets[bucket]['cost'] += cost; buckets[bucket]['tokens'] += tokens
        for group,name in [(groups,prov),(models,model)]:
            g = group.setdefault(name,dict(name=name,cost=0.,tokens=0,sessions=set(),unpriced=0))
            g['cost'] += cost; g['tokens'] += tokens; g['sessions'].add((prov,sid));g['unpriced'] += int(costs is None and reported is None)
    for target,group in [('providers',groups),('models',models)]:
        out[target] = sorted([dict(g,sessions=len(g['sessions'])) for g in group.values()],key=lambda g:-g['tokens'])
    out['sessions'] = len(sessions); out['series'] = list(buckets.values())
    bad = db.execute('select coalesce(sum(bad),0) from files').fetchone()[0]
    if bad: out['warnings'].append(f'{bad} malformed or inconsistent records skipped.')
    if out['unpriced']: out['warnings'].append(f"{out['unpriced']} records have no verified price; their tokens are included, their cost is unknown.")
    out['warnings'].append('Sources: direct Codex, Claude, Grok, OpenCode and Antigravity histories. The existing T3 numerical index supplies compressed logs when available. Copied Codex fork history is suppressed; fresh usage in forks is included.')
    out['warnings'].append('API equivalent estimate, not your subscription bill. Prices use published rate tiers and the current local T3 price cache where available.')
    return out

def main():
    p=argparse.ArgumentParser();p.add_argument('--cache',required=True);p.add_argument('--days',type=int,default=7);p.add_argument('--provider',default='All providers');p.add_argument('--cached',action='store_true');a=p.parse_args()
    os.umask(0o077)
    try: os.nice(10)
    except OSError: pass
    cache=pathlib.Path(a.cache);cache.mkdir(parents=True,exist_ok=True)
    with sqlite3.connect(cache/f'usage-v{VERSION}.sqlite',timeout=1) as db:
        index(db,not a.cached)
        if not a.cached: opencode_index(db)
        if not a.cached: antigravity_index(db)
        if not a.cached: optional_t3_index(db)
        print(json.dumps(report(db,local_rates(pricing(cache,not a.cached)),a.days,a.provider),separators=(',',':')))
if __name__ == '__main__': main()
