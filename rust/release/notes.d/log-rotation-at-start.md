# Host log
- **The host starts even when its log cannot be rotated.** A host log that had reached its size limit and could not be moved aside (another program holding it open) used to stop the host from starting; it now keeps writing to the same file.
