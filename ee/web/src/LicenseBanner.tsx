import { useQuery } from "@tanstack/react-query";
import { meQuery } from "@/api/queries";
import { AdminBanner } from "./AdminBanner";
import { bannerFor, licenseQuery } from "./license";

/** For admins: the license is expiring, in grace, expired or invalid. */
export function LicenseBanner() {
  const { data: me } = useQuery(meQuery);
  const admin = me?.user.role === "admin";
  const { data } = useQuery({ ...licenseQuery, enabled: admin });
  const text = admin && data ? bannerFor(data, Date.now() / 1000) : null;
  if (!text) return null;
  return <AdminBanner text={text} to="/admin/license" link="Manage license" />;
}
