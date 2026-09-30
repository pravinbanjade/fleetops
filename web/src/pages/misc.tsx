import { Link } from "@tanstack/react-router";

export function NotFoundPage() {
  return (
    <div className="mx-auto max-w-md p-10 text-center">
      <h1 className="text-xl font-semibold">Page not found</h1>
      <Link to="/" className="mt-4 inline-block text-sm underline">
        Go to the start page
      </Link>
    </div>
  );
}

export function NoEnvironmentsPage() {
  return (
    <div className="mx-auto max-w-md p-10 text-center">
      <h1 className="text-xl font-semibold">No environments</h1>
      <p className="mt-2 text-sm text-muted-foreground">
        FleetOps registers the local Docker socket when it can reach one at start. Check that the socket is mounted and
        readable, then restart FleetOps.
      </p>
    </div>
  );
}
