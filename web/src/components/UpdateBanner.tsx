import { useEffect, useState } from 'react';
import { t } from '../i18n';
import { parseServerInfo, type ServerInfo } from '../serverInfo';
import { readDismissed, updateAvailable, updateToShow, writeDismissed } from '../updateNotice';

/** The server checks for releases itself; the page reads what it found
 *  (GET /api/server/info) now and then. Administrators only. */
const REFRESH_MS = 60 * 60 * 1000;

export function useServerUpdate(token: string | null, isAdmin: boolean, apiBase: () => string) {
  const [info, setInfo] = useState<ServerInfo | null>(null);
  const [dismissed, setDismissed] = useState<string | null>(readDismissed);
  useEffect(() => {
    if (!token || !isAdmin) { setInfo(null); return; }
    let live = true;
    const load = () =>
      fetch(`${apiBase()}/api/server/info`, { headers: { Authorization: `Bearer ${token}` } })
        .then(r => (r.ok ? r.json() : null))
        .then(body => { if (live && body) setInfo(parseServerInfo(body)); })
        .catch(() => { /* the Server page says why */ });
    load();
    const timer = window.setInterval(load, REFRESH_MS);
    return () => { live = false; window.clearInterval(timer); };
    // (apiBase is the same server for as long as the page is open.)
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [token, isAdmin]);
  const notice = updateToShow(info, dismissed);
  const dismiss = () => {
    if (!notice) return;
    writeDismissed(notice.latest);
    setDismissed(notice.latest);
  };
  return { notice, available: updateAvailable(info), dismiss };
}

type Props = {
  notice: NonNullable<ReturnType<typeof useServerUpdate>['notice']>;
  onDismiss: () => void;
  onOpenServer: () => void;
};

/** "Lunaris X is available" across the top of the administrators' pages. */
export function UpdateBanner({ notice, onDismiss, onOpenServer }: Props) {
  return (
    <div className="update-banner" role="status">
      <svg className="update-banner-icon" width="18" height="18" viewBox="0 0 24 24" fill="none" stroke="currentColor"
        strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">
        <path d="M12 3v12" /><polyline points="7 10 12 15 17 10" /><path d="M5 21h14" />
      </svg>
      <div className="update-banner-text">
        <strong>{t('Lunaris %1 is available', notice.latest)}</strong>
        {notice.current && <span className="update-banner-sub">{t('This server runs %1.', notice.current)}</span>}
      </div>
      <div className="update-banner-actions">
        {notice.url && (
          <a className="btn-secondary update-banner-btn" href={notice.url} target="_blank" rel="noreferrer noopener">
            {t('What is new')}
          </a>
        )}
        <button type="button" className="btn-secondary update-banner-btn" onClick={onOpenServer}>{t('How to update')}</button>
        <button type="button" className="update-banner-close" onClick={onDismiss}
          aria-label={t('Hide until the next release')} title={t('Hide until the next release')}>
          <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" aria-hidden="true">
            <line x1="18" y1="6" x2="6" y2="18" /><line x1="6" y1="6" x2="18" y2="18" />
          </svg>
        </button>
      </div>
    </div>
  );
}
