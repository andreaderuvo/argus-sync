// A pretend app, so the window can be opened in a browser while it is being made and tested
// (tests/ui). Loaded only where there is no window.__TAURI__ — never inside the real app.
const q = new URLSearchParams(location.search);
const listeners = {};
const calls = [];
const at = (m) => `2026-10-14 11:${String(m).padStart(2, '0')}:05`;
let state = {
  computer: 'lap684',
  paused_all: false,
  pairs: q.has('empty') ? [] : [
    {
      id: 'a1', name: 'report-salmonella', url: 'http://lab-server:8090', host: 'lab-server', local: '/Users/sam/argus-sync/report-salmonella',
      paused: false, big_mb: 500,
      status: {
        state: 'idle', message: '', last_ok: at(32), write: true,
        conflicts: ['tables/summary (conflict from lap684 2026-10-14 1128).xlsx'],
        big: [{ path: 'reads/S1_R1.fastq.gz', size: 4.2e9 }, { path: 'reads/S1_R2.fastq.gz', size: 4.4e9 }],
        notes: [{ kind: 'case', kept: 'figures/fig1.pdf', skipped: 'figures/Fig1.pdf' }],
        recent: [
          { at: at(31), kind: 'down', path: 'report/salmonella_2026-10.pdf', detail: '2483112' },
          { at: at(29), kind: 'up', path: 'notes/meeting.md', detail: '4210' },
          { at: at(28), kind: 'conflict', path: 'tables/summary.xlsx', detail: 'tables/summary (conflict from lap684 2026-10-14 1128).xlsx' },
          { at: at(20), kind: 'deleted-there', path: 'old/draft.docx', detail: '' },
          { at: at(12), kind: 'folder-here', path: 'figures', detail: '' },
        ],
      },
    },
    {
      id: 'b2', name: 'listeria-cgmlst', url: 'http://lab-server:8090', host: 'lab-server', local: '/Users/sam/Documents/listeria-cgmlst',
      paused: false, big_mb: 500,
      status: { state: 'syncing', message: '', last_ok: at(30), write: false, conflicts: [], big: [], notes: [], recent: [
        { at: at(32), kind: 'down', path: 'distances/cgdist_matrix.tsv', detail: '18342001' },
      ] },
    },
  ],
};
const emit = (name, payload) => (listeners[name] || []).forEach((f) => f({ payload }));
const find = (id) => state.pairs.find((p) => p.id === id);
window.__TAURI__ = {
  core: {
    async invoke(cmd, args = {}) {
      calls.push([cmd, args]);
      switch (cmd) {
        case 'get_snapshot': return structuredClone(state);
        case 'take_invite': return q.get('invite');
        case 'inspect':
          if (!String(args.link).startsWith('argus-sync://')) throw JSON.stringify({ say: 'That is not a link from Argus — it starts with argus-sync://', values: {} });
          return { url: 'http://lab-server:8090', host: 'lab-server', name: 'wgs-batch-12', folder: 'wgs-batch-12', write: true, suggested: '/Users/sam/argus-sync/wgs-batch-12' };
        case 'connect':
          state.pairs.push({ id: `n${state.pairs.length}`, name: 'wgs-batch-12', url: 'http://lab-server:8090', host: 'lab-server', local: args.local, paused: false, big_mb: 500,
            status: { state: 'starting', message: '', last_ok: null, write: true, conflicts: [], big: [], notes: [], recent: [] } });
          setTimeout(() => emit('snapshot', structuredClone(state)));
          return 'n';
        case 'set_paused': find(args.id).paused = args.paused; find(args.id).status.state = args.paused ? 'paused' : 'idle'; break;
        case 'pause_all': state.paused_all = args.paused; break;
        case 'set_big': find(args.id).big_mb = args.mb; break;
        case 'fetch': find(args.id).status.big = find(args.id).status.big.filter((b) => b.path !== args.path); break;
        case 'resolve': find(args.id).status.conflicts = find(args.id).status.conflicts.filter((c) => c !== args.copy); break;
        case 'disconnect': state.pairs = state.pairs.filter((p) => p.id !== args.id); break;
        case 'move_local': find(args.id).local = args.to; break;
        case 'plugin:dialog|open': return '/Users/sam/Desktop/chosen';
        default: break;
      }
      setTimeout(() => emit('snapshot', structuredClone(state)));
      return null;
    },
  },
  event: {
    async listen(name, f) { (listeners[name] ||= []).push(f); return () => {}; },
  },
};
window.__mock = { calls, emit, get state() { return state; } };
