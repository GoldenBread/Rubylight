#pragma once

#include <stdbool.h>
#include <string.h>
#include <sys/stat.h>

// Input nodes the reader could not use: remembered by name and identity, so the enumeration that
// runs every second skips them instead of opening, checking and logging them again. A node that is
// neither a mouse nor a keyboard is skipped from its first check; one that cannot be opened only
// after EVDEV_OPEN_FAILURES passes in a row, since a hot-plugged node can refuse an open briefly
// (before ueventd sets its owner, mode and label, none of which changes its identity). A node that
// disappears is forgotten at the end of the pass that no longer finds it; one recreated under the
// same name (another device number or inode) is tried again.
#define EVDEV_MAX_REJECTED 64
#define EVDEV_OPEN_FAILURES 5

struct RejectedNode {
    char name[128];
    dev_t rdev;
    ino_t ino;
    // Passes in a row whose open failed, while below EVDEV_OPEN_FAILURES.
    int openFailures;
    bool rejected;
    bool seen;
};

struct RejectedNodes {
    struct RejectedNode nodes[EVDEV_MAX_REJECTED];
    int count;
};

static void rejectedNodesBeginPass(struct RejectedNodes* rejected) {
    for (int i = 0; i < rejected->count; i++) rejected->nodes[i].seen = false;
}

static struct RejectedNode* rejectedNodeNamed(struct RejectedNodes* rejected, const char* name) {
    for (int i = 0; i < rejected->count; i++) {
        if (strcmp(rejected->nodes[i].name, name) == 0) return &rejected->nodes[i];
    }
    return NULL;
}

// Whether this node was found unusable as it is now; a known name counts as seen in this pass.
static bool rejectedNodesSkip(struct RejectedNodes* rejected, const char* name, const struct stat* st) {
    struct RejectedNode* node = rejectedNodeNamed(rejected, name);
    if (node == NULL) return false;
    node->seen = true;
    return node->rejected && node->rdev == st->st_rdev && node->ino == st->st_ino;
}

// The entry for this node as it is now, a new identity starting over; NULL when the list is full.
static struct RejectedNode* rejectedNodeFor(struct RejectedNodes* rejected, const char* name, const struct stat* st) {
    struct RejectedNode* node = rejectedNodeNamed(rejected, name);
    if (node == NULL) {
        if (rejected->count == EVDEV_MAX_REJECTED || strlen(name) >= sizeof(node->name)) return NULL;
        node = &rejected->nodes[rejected->count++];
        strcpy(node->name, name);
        node->openFailures = 0;
        node->rejected = false;
    }
    else if (node->rdev != st->st_rdev || node->ino != st->st_ino) {
        node->openFailures = 0;
        node->rejected = false;
    }
    node->rdev = st->st_rdev;
    node->ino = st->st_ino;
    node->seen = true;
    return node;
}

// The node is neither a mouse nor a keyboard; when the list is full it is tried again.
static void rejectedNodesAdd(struct RejectedNodes* rejected, const char* name, const struct stat* st) {
    struct RejectedNode* node = rejectedNodeFor(rejected, name, st);
    if (node != NULL) node->rejected = true;
}

// The node could not be opened in this pass: skipped once that happened EVDEV_OPEN_FAILURES in a row.
static void rejectedNodesOpenFailed(struct RejectedNodes* rejected, const char* name, const struct stat* st) {
    struct RejectedNode* node = rejectedNodeFor(rejected, name, st);
    if (node != NULL && ++node->openFailures >= EVDEV_OPEN_FAILURES) node->rejected = true;
}

// The node opened: earlier failed opens no longer count.
static void rejectedNodesOpened(struct RejectedNodes* rejected, const char* name) {
    struct RejectedNode* node = rejectedNodeNamed(rejected, name);
    if (node != NULL) node->openFailures = 0;
}

// Forgets the nodes this pass did not find: they disappeared.
static void rejectedNodesEndPass(struct RejectedNodes* rejected) {
    int kept = 0;
    for (int i = 0; i < rejected->count; i++) {
        if (rejected->nodes[i].seen) rejected->nodes[kept++] = rejected->nodes[i];
    }
    rejected->count = kept;
}
