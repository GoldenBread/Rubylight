#pragma once

#include <stdbool.h>
#include <string.h>
#include <sys/stat.h>

// Input nodes the reader opened and could not use (no permission, or neither a mouse nor a
// keyboard): remembered by name and identity, so the enumeration that runs every second skips
// them instead of opening, checking and logging them again. A node that disappears is forgotten
// at the end of the pass that no longer finds it; one recreated under the same name (another
// device number or inode) is tried again.
#define EVDEV_MAX_REJECTED 64

struct RejectedNode {
    char name[128];
    dev_t rdev;
    ino_t ino;
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
    return node->rdev == st->st_rdev && node->ino == st->st_ino;
}

// Remembers a node found unusable (or its new identity); when the list is full it is tried again.
static void rejectedNodesAdd(struct RejectedNodes* rejected, const char* name, const struct stat* st) {
    struct RejectedNode* node = rejectedNodeNamed(rejected, name);
    if (node == NULL) {
        if (rejected->count == EVDEV_MAX_REJECTED || strlen(name) >= sizeof(node->name)) return;
        node = &rejected->nodes[rejected->count++];
        strcpy(node->name, name);
    }
    node->rdev = st->st_rdev;
    node->ino = st->st_ino;
    node->seen = true;
}

// Forgets the nodes this pass did not find: they disappeared.
static void rejectedNodesEndPass(struct RejectedNodes* rejected) {
    int kept = 0;
    for (int i = 0; i < rejected->count; i++) {
        if (rejected->nodes[i].seen) rejected->nodes[kept++] = rejected->nodes[i];
    }
    rejected->count = kept;
}
