/**
 * Best-effort native handle reclamation for short-lived JNI-backed policy objects on Android
 * (API 25+ compatible via {@link PhantomReference} and {@link ReferenceQueue}; {@code java.lang.ref.Cleaner}
 * requires API 33 and cannot be used here). Callers still invoke {@code close()} / {@code destroy()}
 * explicitly at stream end.
 */
package com.limelight.nvstream.jni;

import java.lang.ref.PhantomReference;
import java.lang.ref.ReferenceQueue;
import java.util.Collections;
import java.util.Set;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.function.LongConsumer;

public final class HandleReaper {
    private static final ReferenceQueue<Object> QUEUE = new ReferenceQueue<>();
    private static final Set<HandleRef> ACTIVE = Collections.newSetFromMap(new ConcurrentHashMap<>());

    public static final class Token {
        private final HandleRef ref;
        private final AtomicBoolean closed = new AtomicBoolean(false);

        private Token(HandleRef ref) {
            this.ref = ref;
        }

        public void close() {
            if (closed.compareAndSet(false, true)) {
                ACTIVE.remove(ref);
                ref.clear();
                ref.reclaim();
            }
            drain();
        }
    }

    private static final class HandleRef extends PhantomReference<Object> {
        private final long handle;
        private final LongConsumer destroy;
        private final AtomicBoolean freed = new AtomicBoolean(false);

        HandleRef(Object owner, long handle, LongConsumer destroy) {
            super(owner, QUEUE);
            this.handle = handle;
            this.destroy = destroy;
        }

        void reclaim() {
            if (freed.compareAndSet(false, true) && handle > 0) {
                destroy.accept(handle);
            }
        }
    }

    private HandleReaper() {
    }

    public static Token register(Object owner, long handle, LongConsumer destroy) {
        drain();
        HandleRef ref = new HandleRef(owner, handle, destroy);
        ACTIVE.add(ref);
        return new Token(ref);
    }

    public static void drain() {
        HandleRef ref;
        while ((ref = (HandleRef) QUEUE.poll()) != null) {
            ACTIVE.remove(ref);
            ref.clear();
            ref.reclaim();
        }
    }
}
