import { useQuery } from "@tanstack/react-query";
import { Link } from "@tanstack/react-router";
import { meQuery } from "@/api/queries";
import { bannerFor, licenseQuery } from "./license";

/** For admins: the license is expiring, in grace, expired or invalid. */
export function LicenseBanner() {
  const { data: me } = useQuery(meQuery);
  const admin = me?.user.role === "admin";
  const { data } = useQuery({ ...licenseQuery, enabled: admin });
  const text = admin && data ? bannerFor(data, Date.now() / 1000) : null;
  if (!text) return null;
  return (
    <div role="status" className="border-b border-amber-500/30 bg-amber-500/10 px-4 py-2 text-sm">
      {text}{" "}
      <Link to={"/admin/license" as never} className="font-medium underline underline-offset-2">
        Manage license
      </Link>
    </div>
  );
}
