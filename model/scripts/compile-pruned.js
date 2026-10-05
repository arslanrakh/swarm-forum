#!/usr/bin/env node
// Compiles a Quint module to Quint's JSON IR for Apalache, as `quint compile --target json` does,
// but small enough for this model (README, "Apalache and TLC on a model this size").
//
// `quint compile --target json` writes the whole output with one `JSON.stringify`, which fails
// beyond Node's largest string (about 512 MB) on this model: besides the flattened main module, it
// writes the type of every expression, the lookup table, and the effects of every module it
// loaded, reached or not, and the lookup table repeats each definition it points to. Apalache reads
// only the stage, the modules, the types, and the lookup table, and of a lookup entry only its
// kind, id, and name. This script runs Quint's own stages (load, parse, typecheck, compile with the
// q::init, q::step, and q::inv or q::temporalProps definitions), then keeps:
// - the flattened main module, as Quint gives it;
// - the types of the expressions and definitions that occur in it;
// - the lookup entries of the names that occur in it, reduced to kind, id, and name;
// and writes the JSON piece by piece, so that no string grows beyond one entry.
//
// Usage, from model/:
//   node scripts/compile-pruned.js <spec.qnt> --main <module> [--init <action>] [--step <action>]
//       [--invariant <names>] [--temporal <names>] --out <file.qnt.json>
//
// The names after --invariant and --temporal are comma-separated, as for `quint compile`.

'use strict';
const fs = require('fs');
const path = require('path');

const QUINT = path.join(__dirname, '..', 'node_modules', '@informalsystems', 'quint', 'dist', 'src');
const cmds = require(path.join(QUINT, 'cliCommands.js'));
const JSONbig = require(path.join(__dirname, '..', 'node_modules', 'json-bigint'));
const { replacer } = require(path.join(QUINT, 'jsonHelper.js'));

function usage() {
  process.stderr.write(fs.readFileSync(__filename, 'utf8').split('\n').slice(1, 21).join('\n') + '\n');
  process.exit(2);
}

const argv = process.argv.slice(2);
if (argv.length < 1) usage();
const args = {
  input: argv[0],
  target: 'json',
  flatten: true,
  verbosity: 2,
  init: 'init',
  step: 'step',
};
let out;
for (let i = 1; i < argv.length; i += 2) {
  const k = argv[i];
  const v = argv[i + 1];
  if (v === undefined) usage();
  if (k === '--main') args.main = v;
  else if (k === '--init') args.init = v;
  else if (k === '--step') args.step = v;
  else if (k === '--invariant') args.invariant = v.split(',').join(' and ');
  else if (k === '--temporal') args.temporal = v;
  else if (k === '--out') out = v;
  else usage();
}
if (!out) usage();

/** Every `id` of an IR node in `x` (expressions, definitions, parameters, type annotations). */
function collectIds(x, ids) {
  const stack = [x];
  while (stack.length > 0) {
    const v = stack.pop();
    if (v === null || typeof v !== 'object') continue;
    if (Array.isArray(v)) {
      for (const e of v) stack.push(e);
      continue;
    }
    if (v instanceof Map) {
      for (const e of v.values()) stack.push(e);
      continue;
    }
    if (typeof v.id === 'bigint' || typeof v.id === 'number') ids.add(BigInt(v.id));
    for (const key of Object.keys(v)) {
      if (key !== 'id') stack.push(v[key]);
    }
  }
  return ids;
}

function str(v) {
  return JSONbig.stringify(v, replacer);
}

async function main() {
  const chain = next => prev => prev.asyncChain(next);
  const r = await cmds
    .load(args)
    .then(chain(cmds.parse))
    .then(chain(cmds.typecheck))
    .then(chain(cmds.compile));
  if (r.isLeft()) {
    const { msg, stage } = r.value;
    process.stderr.write(`compile-pruned: ${msg}\n`);
    for (const e of stage.errors || []) process.stderr.write(`  ${e.explanation || str(e)}\n`);
    process.exit(1);
  }
  const compiled = r.value;
  const mainModule = compiled.mainModule;
  const ids = collectIds(mainModule, new Set());

  const fd = fs.openSync(out, 'w');
  const w = s => fs.writeSync(fd, s);
  w(`{"stage":${str(compiled.stage)},"main":${str(compiled.main)},"modules":[`);
  // The module itself: one declaration at a time.
  w(`{"id":${str(mainModule.id)},"name":${str(mainModule.name)},"declarations":[`);
  mainModule.declarations.forEach((d, i) => w((i > 0 ? ',' : '') + str(d)));
  w(']}],"types":{');
  let n = 0;
  let typesKept = 0;
  for (const [id, t] of compiled.types) {
    if (!ids.has(BigInt(id))) continue;
    w((n++ > 0 ? ',' : '') + `"${id}":` + str(t));
    typesKept++;
  }
  w('},"table":{');
  n = 0;
  let tableKept = 0;
  for (const [id, def] of compiled.table) {
    if (!ids.has(BigInt(id))) continue;
    w((n++ > 0 ? ',' : '') + `"${id}":` + str({ kind: def.kind, id: def.id, name: def.name }));
    tableKept++;
  }
  w('}}\n');
  fs.closeSync(fd);
  process.stderr.write(
    `compile-pruned: ${mainModule.declarations.length} declarations, ${typesKept} of ${compiled.types.size} types, ` +
      `${tableKept} of ${compiled.table.size} lookup entries, ${fs.statSync(out).size} bytes in ${out}\n`
  );
}

main().catch(e => {
  process.stderr.write(`compile-pruned: ${e.stack || e}\n`);
  process.exit(1);
});
