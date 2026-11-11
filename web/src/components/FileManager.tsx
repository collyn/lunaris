import { useCallback, useEffect, useMemo, useRef, useState } from 'react';
import { FileSession, humanSize, joinRemote, type Hello, type RemoteDir, type RemoteEntry, type Transfer } from '../fileSession';
import { t } from '../i18n';

// File manager for one remote computer, on its own connection (fileSession.ts):
// browse its folders, upload files and folders (pick or drop), download
// files and folders, new folder / rename / delete, and a transfer queue.

type Props = {
  hostId: string;
  hostName: string;
  wsUrl: string;
  onClose: () => void;
  /** Files to upload as soon as connected (e.g. dropped on the stream), into Downloads. */
  initialUploads?: File[];
};

type Job = Transfer & { file?: File; save?: SaveTarget; cancelled?: boolean };
/** Where a download goes: a file handle (File System Access) or memory. */
type SaveTarget = { kind: 'handle'; handle: FileSystemFileHandle } | { kind: 'blob'; name: string };

type PickedFile = { file: File; rel: string };

const fmtDate = (s: number) => (s ? new Date(s * 1000).toLocaleString() : '');

const canPickDir = () => typeof (window as any).showDirectoryPicker === 'function';
const canSaveAs = () => typeof (window as any).showSaveFilePicker === 'function';

/** Files of a dropped folder (webkitGetAsEntry), with their relative paths. */
async function readEntry(entry: any, prefix: string, out: PickedFile[]) {
  if (entry.isFile) {
    const file: File = await new Promise((res, rej) => entry.file(res, rej));
    out.push({ file, rel: prefix + file.name });
  } else if (entry.isDirectory) {
    const reader = entry.createReader();
    for (;;) {
      const batch: any[] = await new Promise((res, rej) => reader.readEntries(res, rej));
      if (!batch.length) break;
      for (const e of batch) await readEntry(e, prefix + entry.name + '/', out);
    }
  }
}

async function droppedFiles(dt: DataTransfer): Promise<PickedFile[]> {
  const out: PickedFile[] = [];
  const entries = Array.from(dt.items || []).map(i => (i as any).webkitGetAsEntry?.()).filter(Boolean);
  if (entries.length) {
    for (const e of entries) await readEntry(e, '', out);
    return out;
  }
  return Array.from(dt.files).map(f => ({ file: f, rel: f.name }));
}

export default function FileManager({ hostId, hostName, wsUrl, onClose, initialUploads }: Props) {
  const sessionRef = useRef<FileSession | null>(null);
  const [hello, setHello] = useState<Hello | null>(null);
  const [status, setStatus] = useState<string>(t('Connecting…'));
  const [error, setError] = useState<string>('');
  const [dir, setDir] = useState<RemoteDir | null>(null);
  const dirRef = useRef<RemoteDir | null>(null);
  dirRef.current = dir;
  const [pathInput, setPathInput] = useState('');
  const [loading, setLoading] = useState(false);
  const [showHidden, setShowHidden] = useState(false);
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [sort, setSort] = useState<{ key: 'name' | 'size' | 'mtime'; asc: boolean }>({ key: 'name', asc: true });
  const [jobs, setJobs] = useState<Job[]>([]);
  const jobsRef = useRef<Job[]>([]);
  const runningRef = useRef(false);
  const [dragOver, setDragOver] = useState(false);
  const fileInput = useRef<HTMLInputElement | null>(null);
  const folderInput = useRef<HTMLInputElement | null>(null);
  const [, tick] = useState(0);
  const refreshJobs = useCallback(() => { setJobs([...jobsRef.current]); }, []);

  const sep = hello?.sep || '/';

  const load = useCallback(async (path: string, hidden = showHidden) => {
    const s = sessionRef.current;
    if (!s?.open) return;
    setLoading(true);
    setError('');
    try {
      const d = await s.list(path, hidden);
      setDir(d);
      setPathInput(d.path);
      setSelected(new Set());
    } catch (e) {
      setError((e as Error).message);
    } finally {
      setLoading(false);
    }
  }, [showHidden]);

  // Connect once.
  useEffect(() => {
    const s = new FileSession({ wsUrl, hostId, onClose: (reason) => { setStatus(reason); setError(reason); } });
    sessionRef.current = s;
    let alive = true;
    s.connect()
      .then(async (h) => {
        if (!alive) return;
        setHello(h);
        setStatus('');
        const d = await s.list(h.home, false).catch((e) => { setError(e.message); return null; });
        if (d && alive) { setDir(d); setPathInput(d.path); }
        if (initialUploads?.length && alive) {
          enqueueUploads(initialUploads.map(f => ({ file: f, rel: f.name })), joinRemote(h.downloads, 'Lunaris', h.sep), h.sep);
        }
      })
      .catch((e) => { if (alive) { setStatus(''); setError(e.message); } });
    return () => { alive = false; s.close(); };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [wsUrl, hostId]);

  useEffect(() => {
    const onKey = (e: KeyboardEvent) => { if (e.key === 'Escape' && !(e.target instanceof HTMLInputElement)) onClose(); };
    window.addEventListener('keydown', onKey);
    return () => window.removeEventListener('keydown', onKey);
  }, [onClose]);

  // ---------------- transfers

  const runQueue = useCallback(async () => {
    if (runningRef.current) return;
    runningRef.current = true;
    try {
      for (;;) {
        const job = jobsRef.current.find(j => j.state === 'queued');
        const s = sessionRef.current;
        if (!job || !s?.open) break;
        job.state = 'running';
        job.startedAt = performance.now();
        refreshJobs();
        let last = 0;
        const progress = () => {
          const now = performance.now();
          if (now - last > 200) { last = now; refreshJobs(); }
        };
        try {
          if (job.direction === 'up' && job.file) {
            await s.upload(job, job.file, progress, () => !!job.cancelled);
          } else if (job.direction === 'down' && job.save) {
            if (job.save.kind === 'handle') {
              const writable = await (job.save.handle as any).createWritable();
              try {
                await s.download(job, (c) => writable.write(c), progress, () => !!job.cancelled);
              } finally {
                await writable.close();
              }
            } else {
              const parts: ArrayBuffer[] = [];
              await s.download(job, (c) => { parts.push(c); }, progress, () => !!job.cancelled);
              if ((job.state as string) === 'done') {
                const url = URL.createObjectURL(new Blob(parts));
                const a = document.createElement('a');
                a.href = url;
                a.download = job.save.name;
                document.body.appendChild(a);
                a.click();
                a.remove();
                setTimeout(() => URL.revokeObjectURL(url), 30000);
              }
            }
          }
        } catch (e) {
          job.state = 'failed';
          job.message = (e as Error).message;
        }
        refreshJobs();
        // Show what arrived in the open folder.
        const open = dirRef.current;
        if (job.direction === 'up' && open && job.remotePath.startsWith(open.path) && !jobsRef.current.some(j => j.state === 'queued' && j.direction === 'up')) void load(open.path);
      }
    } finally {
      runningRef.current = false;
    }
  }, [refreshJobs, load]);

  const addJobs = (list: Job[]) => {
    jobsRef.current = [...jobsRef.current, ...list];
    refreshJobs();
    void runQueue();
  };

  const newId = (() => { let n = 0; return () => `t${Date.now().toString(36)}${(n += 1)}${Math.random().toString(36).slice(2, 6)}`; })();

  function enqueueUploads(files: PickedFile[], destDir: string, separator = sep) {
    addJobs(files.map(({ file, rel }) => ({
      id: newId(),
      direction: 'up' as const,
      name: rel,
      remotePath: joinRemote(destDir, rel.split('/').join(separator), separator),
      size: file.size,
      done: 0,
      state: 'queued' as const,
      file,
    })));
  }

  const uploadHere = (files: PickedFile[]) => {
    if (!dir) return;
    enqueueUploads(files, dir.path);
  };

  const downloadSelected = async () => {
    const s = sessionRef.current;
    if (!s || !dir) return;
    const items = dir.entries.filter(e => selected.has(e.name));
    if (!items.length) return;
    const list: Job[] = [];
    // With File System Access: pick a folder once and write everything there.
    let folder: any = null;
    if (canPickDir() && (items.length > 1 || items.some(i => i.dir))) {
      try { folder = await (window as any).showDirectoryPicker({ mode: 'readwrite' }); } catch { return; }
    }
    for (const item of items) {
      const remote = joinRemote(dir.path, item.name, sep);
      if (item.dir) {
        let tree;
        try { tree = await s.walk(remote); } catch (e) { setError((e as Error).message); continue; }
        if (!folder && !canPickDir()) {
          // Without folder access each file is saved on its own.
          for (const f of tree.files) list.push(downloadJob(remote, f.rel, f.size, { kind: 'blob', name: `${item.name}_${f.rel.replace(/\//g, '_')}` }));
          continue;
        }
        const top = await folder.getDirectoryHandle(item.name, { create: true });
        for (const d of tree.dirs) await dirHandle(top, d);
        for (const f of tree.files) {
          const parts = f.rel.split('/');
          const parent = await dirHandle(top, parts.slice(0, -1).join('/'));
          const handle = await parent.getFileHandle(parts[parts.length - 1], { create: true });
          list.push(downloadJob(remote, f.rel, f.size, { kind: 'handle', handle }));
        }
      } else if (folder) {
        const handle = await folder.getFileHandle(item.name, { create: true });
        list.push(downloadJob(dir.path, item.name, item.size, { kind: 'handle', handle }));
      } else if (canSaveAs()) {
        let handle;
        try { handle = await (window as any).showSaveFilePicker({ suggestedName: item.name }); } catch { continue; }
        list.push(downloadJob(dir.path, item.name, item.size, { kind: 'handle', handle }));
      } else {
        list.push(downloadJob(dir.path, item.name, item.size, { kind: 'blob', name: item.name }));
      }
    }
    addJobs(list);
  };

  async function dirHandle(root: any, rel: string) {
    let h = root;
    for (const part of rel.split('/').filter(Boolean)) h = await h.getDirectoryHandle(part, { create: true });
    return h;
  }

  function downloadJob(baseDir: string, rel: string, size: number, save: SaveTarget): Job {
    return {
      id: newId(),
      direction: 'down',
      name: rel,
      remotePath: joinRemote(baseDir, rel.split('/').join(sep), sep),
      size,
      done: 0,
      state: 'queued',
      save,
    };
  }

  // ---------------- file operations

  const openEntry = (e: RemoteEntry) => {
    if (!dir) return;
    if (e.dir) void load(joinRemote(dir.path, e.name, sep));
    else { setSelected(new Set([e.name])); }
  };

  const mkdir = async () => {
    const s = sessionRef.current;
    if (!s || !dir) return;
    const name = window.prompt(t('New folder name'), t('New folder'));
    if (!name) return;
    try { await s.mkdir(joinRemote(dir.path, name, sep)); await load(dir.path); } catch (e) { setError((e as Error).message); }
  };

  const rename = async () => {
    const s = sessionRef.current;
    if (!s || !dir || selected.size !== 1) return;
    const old = Array.from(selected)[0];
    const name = window.prompt(t('Rename to'), old);
    if (!name || name === old) return;
    try { await s.rename(joinRemote(dir.path, old, sep), name); await load(dir.path); } catch (e) { setError((e as Error).message); }
  };

  const remove = async () => {
    const s = sessionRef.current;
    if (!s || !dir || !selected.size) return;
    const names = Array.from(selected);
    if (!window.confirm(t('Delete %1 item(s) on %2? This cannot be undone.', names.length, hostName) + '\n\n' + names.slice(0, 8).join('\n') + (names.length > 8 ? '\n…' : ''))) return;
    for (const n of names) {
      try { await s.remove(joinRemote(dir.path, n, sep)); } catch (e) { setError((e as Error).message); break; }
    }
    await load(dir.path);
  };

  // ---------------- view

  const entries = useMemo(() => {
    const list = [...(dir?.entries || [])];
    const dirFirst = (a: RemoteEntry, b: RemoteEntry) => Number(b.dir) - Number(a.dir);
    list.sort((a, b) => {
      const d = dirFirst(a, b);
      if (d) return d;
      let c = 0;
      if (sort.key === 'name') c = a.name.localeCompare(b.name, undefined, { numeric: true, sensitivity: 'base' });
      else if (sort.key === 'size') c = a.size - b.size;
      else c = a.mtime - b.mtime;
      return sort.asc ? c : -c;
    });
    return list;
  }, [dir, sort]);

  const toggle = (name: string, e: React.MouseEvent) => {
    setSelected(prev => {
      const next = new Set(e.ctrlKey || e.metaKey ? prev : []);
      if (e.shiftKey && prev.size) {
        const names = entries.map(x => x.name);
        const anchor = names.indexOf(Array.from(prev).pop()!);
        const to = names.indexOf(name);
        for (let i = Math.min(anchor, to); i <= Math.max(anchor, to); i++) next.add(names[i]);
        return next;
      }
      if (prev.has(name) && (e.ctrlKey || e.metaKey)) next.delete(name); else next.add(name);
      return next;
    });
  };

  const active = jobs.filter(j => j.state === 'running' || j.state === 'queued').length;
  const speed = (j: Job) => {
    if (j.state !== 'running' || !j.startedAt) return '';
    const secs = (performance.now() - j.startedAt) / 1000;
    return secs > 0.5 ? `${humanSize(j.done / secs)}/s` : '';
  };

  useEffect(() => {
    if (!active) return;
    const i = window.setInterval(() => tick(x => x + 1), 500);
    return () => window.clearInterval(i);
  }, [active]);

  const sortHeader = (key: 'name' | 'size' | 'mtime', label: string) => (
    <button type="button" className="fm-th" onClick={() => setSort(s => ({ key, asc: s.key === key ? !s.asc : true }))}>
      {label}{sort.key === key ? (sort.asc ? ' ↑' : ' ↓') : ''}
    </button>
  );

  const Icon = ({ d }: { d: string }) => (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true"><path d={d} /></svg>
  );
  const FOLDER = 'M3 6a1 1 0 0 1 1-1h5l2 2h9a1 1 0 0 1 1 1v10a1 1 0 0 1-1 1H4a1 1 0 0 1-1-1z';
  const FILE = 'M6 3h8l4 4v14H6z M14 3v4h4';

  return (
    <div
      className="ssd-overlay"
      onMouseDown={(e) => { if (e.target === e.currentTarget) onClose(); }}
      // Drops here are ours, not the stream's (this may sit over it).
      onDragOver={(e) => { e.stopPropagation(); e.preventDefault(); }}
      onDragLeave={(e) => e.stopPropagation()}
      onDrop={(e) => { e.stopPropagation(); e.preventDefault(); }}
    >
      <div className="ssd fm" role="dialog" aria-modal="true" aria-label={t('Files')}>
        <header className="ssd-header">
          <div className="ssd-breadcrumb">
            <span className="ssd-crumb-current">{t('Files')}</span>
            <span className="ssd-host">{hostName}</span>
            {status && <span className="fm-status">{status}</span>}
          </div>
          <button type="button" className="ssd-close" onClick={onClose} aria-label={t('Close')} title={t('Close')}>
            <svg width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round"><path d="M6 6l12 12M18 6L6 18" /></svg>
          </button>
        </header>

        <div className="ssd-body">
          <nav className="ssd-nav fm-places">
            {(hello?.roots || []).map(r => (
              <button key={r.path} type="button" className={`ssd-nav-item${dir?.path === r.path ? ' active' : ''}`} onClick={() => void load(r.path)} title={r.path}>
                <Icon d={FOLDER} />
                <span>{t(r.name)}</span>
              </button>
            ))}
          </nav>

          <div
            className={`fm-main${dragOver ? ' drag' : ''}`}
            onDragOver={(e) => { if (e.dataTransfer.types.includes('Files')) { e.preventDefault(); setDragOver(true); } }}
            onDragLeave={(e) => { if (e.currentTarget === e.target) setDragOver(false); }}
            onDrop={async (e) => { e.preventDefault(); setDragOver(false); uploadHere(await droppedFiles(e.dataTransfer)); }}
          >
            <div className="fm-toolbar">
              <button type="button" className="icon-btn" disabled={!dir?.parent} onClick={() => dir?.parent && void load(dir.parent)} title={t('Up one folder')}><Icon d="M12 19V5 M5 12l7-7 7 7" /></button>
              <button type="button" className="icon-btn" disabled={!dir} onClick={() => dir && void load(dir.path)} title={t('Refresh')}><Icon d="M20 12a8 8 0 1 1-2.3-5.6 M20 4v5h-5" /></button>
              <form className="fm-path" onSubmit={(e) => { e.preventDefault(); void load(pathInput); }}>
                <input value={pathInput} onChange={(e) => setPathInput(e.target.value)} aria-label={t('Folder on the remote computer')} spellCheck={false} />
              </form>
              <label className="fm-hidden"><input type="checkbox" checked={showHidden} onChange={(e) => { setShowHidden(e.target.checked); if (dir) void load(dir.path, e.target.checked); }} /> {t('Hidden files')}</label>
            </div>
            <div className="fm-actions">
              <button type="button" className="btn-primary" disabled={!dir} onClick={() => fileInput.current?.click()}>{t('Upload files')}</button>
              <button type="button" className="btn-secondary" disabled={!dir} onClick={() => folderInput.current?.click()}>{t('Upload folder')}</button>
              <button type="button" className="btn-secondary" disabled={!selected.size} onClick={() => void downloadSelected()}>{t('Download')}</button>
              <span className="fm-sep" />
              <button type="button" className="btn-secondary" disabled={!dir} onClick={() => void mkdir()}>{t('New folder')}</button>
              <button type="button" className="btn-secondary" disabled={selected.size !== 1} onClick={() => void rename()}>{t('Rename')}</button>
              <button type="button" className="btn-secondary fm-danger" disabled={!selected.size} onClick={() => void remove()}>{t('Delete')}</button>
              <input ref={fileInput} type="file" multiple hidden onChange={(e) => { const fs = Array.from(e.target.files || []); e.target.value = ''; uploadHere(fs.map(f => ({ file: f, rel: f.name }))); }} />
              <input ref={folderInput} type="file" multiple hidden {...({ webkitdirectory: '' } as any)} onChange={(e) => { const fs = Array.from(e.target.files || []); e.target.value = ''; uploadHere(fs.map(f => ({ file: f, rel: (f as any).webkitRelativePath || f.name }))); }} />
            </div>

            {error && <div className="fm-error" role="alert">{error}</div>}

            <div className="fm-table" role="grid" aria-busy={loading}>
              <div className="fm-row fm-head">
                <span />
                {sortHeader('name', t('Name'))}
                {sortHeader('size', t('Size'))}
                {sortHeader('mtime', t('Modified'))}
              </div>
              {entries.map(e => (
                <div
                  key={e.name}
                  className={`fm-row${selected.has(e.name) ? ' selected' : ''}${e.hidden ? ' hidden-entry' : ''}`}
                  onClick={(ev) => toggle(e.name, ev)}
                  onDoubleClick={() => openEntry(e)}
                  role="row"
                >
                  <span className={e.dir ? 'fm-icon dir' : 'fm-icon'}><Icon d={e.dir ? FOLDER : FILE} /></span>
                  <span className="fm-name" title={e.name}>{e.name}</span>
                  <span className="fm-size">{e.dir ? '' : humanSize(e.size)}</span>
                  <span className="fm-date">{fmtDate(e.mtime)}</span>
                </div>
              ))}
              {dir && !entries.length && !loading && <div className="fm-empty">{t('This folder is empty. Drop files here to upload them.')}</div>}
              {!dir && !error && <div className="fm-empty">{status || t('Loading…')}</div>}
            </div>
            {dragOver && <div className="fm-drop">{t('Drop to upload to %1', dir?.path || '')}</div>}
          </div>
        </div>

        <footer className="fm-transfers">
          <div className="fm-transfers-head">
            <strong>{t('Transfers')}</strong>
            <span className="fm-muted">{active ? t('%1 in progress', active) : jobs.length ? t('All done') : t('Nothing yet')}</span>
            {jobs.some(j => j.state !== 'running' && j.state !== 'queued') && (
              <button type="button" className="fm-link" onClick={() => { jobsRef.current = jobsRef.current.filter(j => j.state === 'running' || j.state === 'queued'); refreshJobs(); }}>{t('Clear finished')}</button>
            )}
          </div>
          {jobs.length > 0 && (
            <div className="fm-jobs">
              {[...jobs].reverse().map(j => (
                <div key={j.id} className={`fm-job ${j.state}`}>
                  <span className="fm-dir" aria-hidden="true">{j.direction === 'up' ? '↑' : '↓'}</span>
                  <span className="fm-job-name" title={j.remotePath}>{j.name}</span>
                  <div className="fm-bar"><div style={{ width: `${j.size ? Math.min(100, (j.done * 100) / j.size) : j.state === 'done' ? 100 : 0}%` }} /></div>
                  <span className="fm-job-info">
                    {j.state === 'running' && `${humanSize(j.done)} / ${humanSize(j.size)} ${speed(j)}`}
                    {j.state === 'queued' && t('Waiting')}
                    {j.state === 'done' && t('Done')}
                    {j.state === 'skipped' && (j.message || t('Skipped'))}
                    {j.state === 'cancelled' && t('Cancelled')}
                    {j.state === 'failed' && <span className="fm-fail" title={j.message}>{j.message || t('Failed')}</span>}
                  </span>
                  {(j.state === 'running' || j.state === 'queued') && (
                    <button type="button" className="fm-link" onClick={() => {
                      j.cancelled = true;
                      if (j.state === 'queued') j.state = 'cancelled';
                      refreshJobs();
                    }}>{t('Cancel')}</button>
                  )}
                </div>
              ))}
            </div>
          )}
        </footer>
      </div>
    </div>
  );
}
