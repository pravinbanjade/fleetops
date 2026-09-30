import { toast } from "sonner";
import { ApiError } from "@/api/client";

/**
 * The text to show for a failed request: the server's message (Docker's
 * own, for upstream errors), plus the request id for internal errors so a
 * user can find the server log line.
 */
export function errorMessage(err: unknown): string {
  if (err instanceof ApiError) {
    return err.code === "internal" && err.requestId ? `${err.message} (request ${err.requestId})` : err.message;
  }
  if (err instanceof Error) return err.message;
  return String(err);
}

export function toastError(err: unknown) {
  toast.error(errorMessage(err));
}
