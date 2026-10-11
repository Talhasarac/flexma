// Download a full-server backup and see how to restore it (admins only).
import { useEffect, useState } from 'react';
import { Download, Loader2 } from 'lucide-react';
import { api, type BackupInfo } from '../../lib/api';
import { formatBytes } from '../../lib/fonts';
import { CopyField } from '../../components/CopyField';
import { SettingsSection } from '../../components/SettingsSection';
import { useToast } from '../../components/Toast';

export function BackupSection() {
  const toast = useToast();
  const [info, setInfo] = useState<BackupInfo | null>(null);
  const [preparing, setPreparing] = useState(false);

  useEffect(() => {
    api.admin.backupInfo().then(setInfo, () => undefined);
  }, []);

  const plural = (n: number, one: string) => `${n} ${one}${n === 1 ? '' : 's'}`;

  return (
    <SettingsSection
      title="Backups"
      description="Download everything on this server as one .zip: accounts, workspaces, every design with its version history and comments, uploaded images and fonts."
    >
      <div className="stack">
        {info && (
          <p className="text-muted text-sm">
            Database {formatBytes(info.databaseBytes)} · {plural(info.assetsFiles, 'image')} ({formatBytes(info.assetsBytes)}) ·{' '}
            {plural(info.fontsFiles, 'font file')} ({formatBytes(info.fontsBytes)})
          </p>
        )}
        <div>
          <a
            className="btn btn-primary"
            href={api.admin.backupUrl}
            download
            onClick={() => {
              setPreparing(true);
              toast.info('Preparing your backup. The download starts in a moment.', { id: 'backup' });
              window.setTimeout(() => setPreparing(false), 4000);
            }}
          >
            {preparing ? <Loader2 size={15} className="spin" /> : <Download size={15} />} Download backup
          </a>
        </div>
        <p className="text-muted text-sm">
          It includes password hashes and login tokens, so keep it somewhere private. Only admins signed in here can download
          it; API and agent tokens can't.
        </p>
        <details className="backup-restore">
          <summary>How to restore a backup</summary>
          <div className="stack">
            <p className="text-sm">1. Copy the zip to the server:</p>
            <CopyField value="scp flexma-backup-*.zip root@YOUR-SERVER:/root/" toastMessage="Command copied" />
            <p className="text-sm">2. On the server, stop Flexma, restore, and start it again:</p>
            <CopyField
              multiline
              value={'systemctl stop flexma\nFLEXMA_DATA_DIR=/var/lib/flexma /opt/flexma/flexma restore /root/flexma-backup-*.zip\nsystemctl start flexma'}
              toastMessage="Commands copied"
            />
            <p className="text-muted text-sm">
              The data it replaces is kept in a <code>pre-restore-…</code> folder next to it, so a restore can be undone.
            </p>
          </div>
        </details>
      </div>
    </SettingsSection>
  );
}
