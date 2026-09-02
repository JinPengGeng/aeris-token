import assert from 'node:assert/strict';
import { readFile } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import test from 'node:test';

const workflowPath = fileURLToPath(new URL('../../workflows/rust-ci.yml', import.meta.url));
const expectedActions = new Map([
  ['actions/checkout', 'fbc6f3992d24b796d5a048ff273f7fcc4a7b6c09'],
  ['dtolnay/rust-toolchain', '4360b52568e2003a75bf9bc1d59f33a8e3fc893c'],
  ['Swatinem/rust-cache', '6323deb102c322ba6fcbdcafc7e3dddab59af2b6'],
  ['mozilla-actions/sccache-action', 'fc920bf0ec8de6ee65d409111f7ec508035751ba'],
  ['rui314/setup-mold', '9c9c13bf4c3f1adef0cc596abc155580bcb04444'],
  ['taiki-e/install-action', 'd5f9268ff7620505a81ada10ddf18cdd72240185'],
  ['dorny/paths-filter', 'de90cc6fb38fc0963ad72b210f1f284cd68cea36'],
]);

test('Rust CI pins every third-party action to its approved immutable commit', async () => {
  const workflow = await readFile(workflowPath, 'utf8');
  const actionRefs = [...workflow.matchAll(/^\s*-?\s*uses:\s*([^\s#]+)(?:\s+#.*)?$/gm)]
    .map((match) => match[1]);

  assert.ok(actionRefs.length > 0, 'Rust CI should invoke third-party actions');
  for (const ref of actionRefs) {
    const match = /^(?<action>[^@]+)@(?<sha>[0-9a-f]{40})$/.exec(ref);
    assert.ok(match, `action reference must use a full 40-character SHA: ${ref}`);
    assert.equal(expectedActions.get(match.groups.action), match.groups.sha, `unexpected action SHA: ${ref}`);
  }
  assert.deepEqual(new Set(actionRefs.map((ref) => ref.split('@')[0])), new Set(expectedActions.keys()));
});
