#!/usr/bin/env python3
"""Verify ZIP hashes and exercise its executable offline; never installs it."""
import argparse,hashlib,json,os,queue,stat,subprocess,tempfile,threading,uuid,zipfile
from pathlib import Path,PurePosixPath
OPEN={'amphetamine':'amp-duration','audio-device':'customize-order','cleanshotx':'manage-recording-presets','google-search':'index','speedtest':'index'}
def smoke(path):
    with tempfile.TemporaryDirectory() as temp,zipfile.ZipFile(path) as archive:
        root=Path(temp)/'package';root.mkdir()
        names=archive.namelist();assert len(names)==len(set(names)),'Duplicate ZIP paths'
        for item in archive.infolist():
            name=PurePosixPath(item.filename)
            assert not name.is_absolute() and '..' not in name.parts and '\\' not in item.filename and ':' not in item.filename,'Unsafe ZIP path'
            assert not stat.S_ISLNK(item.external_attr>>16),'Symlink ZIP entry'
        manifest=json.loads(archive.read('extension.json'));inventory=manifest['files']
        assert set(names)==set(inventory)|{'extension.json'},'ZIP/inventory mismatch'
        for name,digest in inventory.items():assert hashlib.sha256(archive.read(name)).hexdigest()==digest,'SHA inventory mismatch: '+name
        archive.extractall(root)
        slug=manifest['slug'];binary=root/'bin'/('tezbar-'+slug+('.exe' if os.name=='nt' else ''));binary.chmod(0o755)
        env={k:v for k,v in os.environ.items() if k in ('SystemRoot','WINDIR','TEMP','TMP')}
        env['PATH']=os.path.join(env.get('SystemRoot','C:/Windows'),'System32') if os.name=='nt' else '/usr/bin:/bin'
        info=json.loads(subprocess.check_output([str(binary),'--protocol-info'],cwd=root,env=env,timeout=10));assert info['id']==manifest['id']
        process=subprocess.Popen([str(binary)],cwd=root,env=env,stdin=subprocess.PIPE,stdout=subprocess.PIPE,stderr=None)
        messages=queue.Queue()
        def reader():
            try:
                while True:
                    line=process.stdout.readline(16*1024*1024+1)
                    if not line:messages.put(EOFError('Unexpected EOF'));return
                    if len(line)>16*1024*1024:raise ValueError('Oversized frame')
                    messages.put(json.loads(line))
            except Exception as error:messages.put(error)
        threading.Thread(target=reader,daemon=True).start()
        def receive():
            value=messages.get(timeout=8)
            if isinstance(value,Exception):raise value
            return value
        def request(method,params):
            ident='h:'+str(uuid.uuid4());value={'type':'request','id':ident,'method':method,'params':params}
            process.stdin.write((json.dumps(value)+'\n').encode());process.stdin.flush()
            while True:
                frame=receive()
                if frame['type']=='response' and frame['id']==ident:
                    assert 'error' not in frame,frame
                    return frame['result']
                assert frame['type']=='event','Safe view must not request host/hardware/provider effects: '+str(frame)
        try:
            hello=receive();assert hello['type']=='hello' and hello['id']==manifest['id'] and hello['version']==manifest['version'] and hello['protocol']==1
            request('initialize',{'protocol':1,'extensionId':manifest['id'],'platform':'Windows' if os.name=='nt' else 'macOS','packageDir':str(root),'dataDir':str(Path(temp)/'data'),'tempDir':str(Path(temp)/'temp'),'grantedCapabilities':manifest['capabilities'],'reason':'command','migrationPaths':{}})
            view=request('session.open',{'sessionId':'zip-smoke','commandName':OPEN[slug],'preferences':{},'argumentValues':{}});assert view['mode']=='view'
            request('session.dispose',{'sessionId':'zip-smoke'})
            request('shutdown',{})
            assert process.wait(timeout=5)==0,'Process did not exit cleanly'
        finally:
            if process.poll() is None:process.kill();process.wait(timeout=5)
            process.stdin.close();process.stdout.close()
        print(slug,'ZIP inventory + protocol-info/hello/initialize/open/dispose/shutdown passed (offline, no installation)')
if __name__=='__main__':
    p=argparse.ArgumentParser();p.add_argument('archive',type=Path);a=p.parse_args();smoke(a.archive)
