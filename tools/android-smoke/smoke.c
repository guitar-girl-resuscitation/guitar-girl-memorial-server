#include <jni.h>
#include <android/asset_manager_jni.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>
#include "ggfm_server.h"

JNIEXPORT jstring JNICALL
Java_org_guitargirlresuscitation_abi_1smoke_SmokeActivity_runSmoke(
    JNIEnv *env, jobject self, jobject assets, jstring directory) {
    (void)self;
    const char *path = (*env)->GetStringUTFChars(env, directory, NULL);
    if (!path) return NULL;
    AAssetManager *manager = AAssetManager_fromJava(env, assets);
    const int64_t now = (int64_t)time(NULL);
    char report[8192];
    int used = snprintf(report, sizeof(report), "pointer_bits=%zu abi=%u\n",
                        sizeof(void*) * 8, ggfm_server_abi_version());
    int64_t previous_usn = 0;
    int passed = ggfm_server_abi_version() == 1;
    for (int iteration = 0; iteration < 2; ++iteration) {
        unsigned char random_bytes[32];
        char capability[65];
        arc4random_buf(random_bytes, sizeof(random_bytes));
        for (size_t i = 0; i < sizeof(random_bytes); ++i)
            snprintf(capability + i * 2, 3, "%02x", random_bytes[i]);
        int start = ggfm_server_start(path, manager, now + iteration, 600, capability);
        int login = start == 0 ? ggfm_server_begin_login(now + iteration, 600) : -999;
        int64_t usn = start == 0 ? ggfm_server_active_usn() : 0;
        const char *endpoint = start == 0 ? ggfm_server_endpoint() : NULL;
        used += snprintf(report + used, sizeof(report) - used,
            "cycle=%d start=%d login=%d usn=%lld endpoint=%s\n",
            iteration, start, login, (long long)usn, endpoint ? endpoint : "null");
        passed &= start == 0 && login == 0 && usn > 0 && endpoint != NULL;
        if (iteration) passed &= usn == previous_usn;
        previous_usn = usn;
        const int prepare = start == 0 ? ggfm_server_prepare_shutdown() : -999;
        const int stop = ggfm_server_stop();
        passed &= prepare == 0 && stop == 0;
        used += snprintf(report + used, sizeof(report) - used,
                         "prepare=%d stop=%d\n", prepare, stop);
    }
    used += snprintf(report + used, sizeof(report) - used, "RESULT=%s\n", passed ? "PASS" : "FAIL");
    ggfm_server_drain_logs(report + used, sizeof(report) - used);
    report[sizeof(report) - 1] = '\0';
    (*env)->ReleaseStringUTFChars(env, directory, path);
    return (*env)->NewStringUTF(env, report);
}
