package org.guitargirlresuscitation.abi_smoke;

import android.app.Activity;
import android.os.Bundle;
import android.content.res.AssetManager;
import android.widget.TextView;

public final class SmokeActivity extends Activity {
    private native String runSmoke(AssetManager assets, String directory);
    @Override public void onCreate(Bundle state) {
        super.onCreate(state);
        final TextView text = new TextView(this);
        text.setText("Running isolated ABI smoke test...");
        setContentView(text);
        new Thread(new Runnable() { @Override public void run() {
            String report;
            try {
                System.loadLibrary("ggfm_android_ffi");
                System.loadLibrary("ggfm_smoke");
                report = runSmoke(getAssets(), getFilesDir().getAbsolutePath());
            } catch (Throwable error) {
                report = android.util.Log.getStackTraceString(error);
            }
            android.util.Log.i("GGFM_ABI_SMOKE", report);
            final String result = report;
            runOnUiThread(new Runnable() { @Override public void run() { text.setText(result); } });
        } }, "ggfm-abi-smoke").start();
    }
}
