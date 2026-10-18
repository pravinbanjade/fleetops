import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { meQuery } from "@/api/queries";
import { backupBanner, statusQuery } from "./backup";
import { LicenseBanner } from "./LicenseBanner";

/** For admins: scheduled backups are paused or the last one failed. */
export function BackupBanner() {
  const { data: me } = useQuery(meQuery);
  const admin = me?.user.role === "admin";
  const { data } = useQuery({ ...statusQuery, enabled: admin, refetchInterval: 5 * 60_000 });
  const text = admin && data ? backupBanner(data) : null;
  if (!text) return null;
  return (
    <div role="status" className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm">
      {text}{" "}
      <Link to={"/admin/backup" as never} className="font-medium underline underline-offset-2">
        Backup settings
      </Link>
    </div>
  );
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
