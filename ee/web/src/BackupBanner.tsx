import { useQuery } from "@tanstack/react-query";
import { meQuery } from "@/api/queries";
import { AdminBanner } from "./AdminBanner";
import { backupBanner, statusQuery } from "./backup";
import { LicenseBanner } from "./LicenseBanner";

/** For admins: scheduled backups are paused or the last one failed. */
export function BackupBanner() {
  const { data: me } = useQuery(meQuery);
  const admin = me?.user.role === "admin";
  const { data } = useQuery({ ...statusQuery, enabled: admin, refetchInterval: 5 * 60_000 });
  const text = admin && data ? backupBanner(data) : null;
  if (!text) return null;
  return <AdminBanner text={text} to="/admin/backup" link="Backup settings" />;
}

/** Every Enterprise banner, in order. */
export function EeBanners() {
  return (
    <>
      <LicenseBanner />
      <BackupBanner />
    </>
  );
}
