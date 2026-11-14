# Keep JNI entry points and the native bridge.
-keep class com.rkb.rust.RkbNative { *; }
-keepclasseswithmembernames class * {
    native <methods>;
}
