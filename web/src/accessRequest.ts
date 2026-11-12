// The agent's answer when a connection has to be accepted by the person at
// the remote computer (it was added without its password): see
// agent/src/approval.rs and SignalingMessage::AccessRequest.
import { t } from './i18n';

/** Seconds the person there has to answer (agent `approval::TIMEOUT`). */
export const ACCESS_REQUEST_TIMEOUT_S = 30;

/** Why the connection can't go on, or null while it may ("waiting", "accepted"). */
export function accessRefusal(status: string): string | null {
  switch (status) {
    case 'waiting':
    case 'accepted':
      return null;
    case 'denied':
      return t('The person at the remote computer declined the connection.');
    case 'timeout':
      return t('Nobody accepted the connection at the remote computer in time.');
    case 'unattended':
      return t('Nobody can accept connections at the remote computer: add it with its password.');
    default:
      return t('The remote computer refused the connection.');
  }
}
