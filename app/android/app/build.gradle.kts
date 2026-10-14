plugins {
    id("com.android.application")
    // The Flutter Gradle Plugin must be applied after the Android and Kotlin Gradle plugins.
    id("dev.flutter.flutter-gradle-plugin")
}

// Google OAuth client for the mobile apps (Android or iOS application type).
// Its reversed id is the private-use URI scheme that receives the sign-in
// redirect. Read from the environment (CI) or from the build hook's
// user-defines in pubspec.yaml, the same value the Rust engine embeds.
val mobileClientId: String =
    System.getenv("KEYROAM_GOOGLE_MOBILE_CLIENT_ID")?.trim()?.takeIf { it.isNotEmpty() }
        ?: Regex("google_mobile_client_id:\\s*\"?([^\"\\s]+)\"?")
            .find(file("../../pubspec.yaml").readText())
            ?.groupValues
            ?.get(1)
        ?: ""
val oauthRedirectScheme: String =
    mobileClientId.removeSuffix(".apps.googleusercontent.com")
        .takeIf { mobileClientId.endsWith(".apps.googleusercontent.com") && it.isNotEmpty() }
        ?.let { "com.googleusercontent.apps.$it" }
        ?: "io.github.ravipurohit1991.keyroam.oauth-unconfigured"

android {
    namespace = "io.github.ravipurohit1991.keyroam"
    compileSdk = flutter.compileSdkVersion
    ndkVersion = flutter.ndkVersion

    compileOptions {
        sourceCompatibility = JavaVersion.VERSION_17
        targetCompatibility = JavaVersion.VERSION_17
    }

    defaultConfig {
        applicationId = "io.github.ravipurohit1991.keyroam"
        minSdk = flutter.minSdkVersion
        targetSdk = flutter.targetSdkVersion
        // Uses the version code from pubspec.yaml. When using split APKs, 1000 * ABI_VERSION
        // is added automatically by Flutter. (https://developer.android.com/studio/build/configure-apk-splits#configure-APK-versions)
        versionCode = flutter.versionCode
        versionName = flutter.versionName
        manifestPlaceholders["oauthRedirectScheme"] = oauthRedirectScheme
    }

    buildTypes {
        release {
            // TODO: Add your own signing config for the release build.
            // Signing with the debug keys for now, so `flutter run --release` works.
            signingConfig = signingConfigs.getByName("debug")
        }
    }
}

kotlin {
    compilerOptions {
        jvmTarget = org.jetbrains.kotlin.gradle.dsl.JvmTarget.JVM_17
    }
}

flutter {
    source = "../.."
}

dependencies {
    // Background sync and the connectivity trigger.
    implementation("androidx.work:work-runtime-ktx:2.10.1")
    // Biometric unlock: a Keystore key behind a biometric prompt.
    implementation("androidx.biometric:biometric:1.1.0")
    // Credential Manager provider (Android 14+).
    implementation("androidx.credentials:credentials:1.5.0")
}
