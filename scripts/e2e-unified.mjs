// Compatibility entry point for unified API acceptance on a synced build host.
// The Python harness owns one Rust process, its PostgreSQL and synthetic peers.
import {spawnSync} from 'node:child_process'
import {fileURLToPath} from 'node:url'
const script=fileURLToPath(new URL('./e2e-api-contract.py',import.meta.url))
const result=spawnSync('python3',[script,...process.argv.slice(2)],{stdio:'inherit',env:process.env})
if(result.error)throw result.error
process.exit(result.status??1)
