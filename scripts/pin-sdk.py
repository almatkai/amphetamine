#!/usr/bin/env python3
"""Parent-run publication finalization. Never resolves a floating branch."""
import argparse,re,subprocess
from pathlib import Path
root=Path(__file__).resolve().parents[1]
p=argparse.ArgumentParser();p.add_argument('--rev',required=True);p.add_argument('--repository',default='https://github.com/almatkai/tezbar-extension-sdk-rust');a=p.parse_args()
if not re.fullmatch('[0-9a-fA-F]{40}',a.rev):raise SystemExit('SDK revision must be a real full 40-character commit SHA')
if not a.repository.startswith('https://github.com/'):raise SystemExit('Expected approved HTTPS GitHub SDK repository')
path=root/'Cargo.toml';text=path.read_text();text,n=re.subn(r'tezbar-extension-sdk = \{[^\n]*\}',f'tezbar-extension-sdk = {{ git = "{a.repository}", rev = "{a.rev}" }}',text);assert n==1
path.write_text(text);subprocess.run(['cargo','update','--workspace'],cwd=root,check=True,timeout=180)
print('Inspect the full Cargo.toml/Cargo.lock diff, then run cargo test --locked -j1 before publishing.')
