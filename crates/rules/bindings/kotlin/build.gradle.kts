// gradle run -Pbindings=<generated Kotlin dir> -Plibrary=<dir with the host library> -Pfixture=<day_verdicts.json>
plugins {
    kotlin("jvm") version "2.4.20"
    application
}

// The versions the Android fork uses.
dependencies {
    implementation("net.java.dev.jna:jna:5.19.1")
    implementation("org.json:json:20260719")
}

sourceSets.main {
    kotlin.srcDir(providers.gradleProperty("bindings"))
}

kotlin {
    jvmToolchain(21)
}

application {
    mainClass = "DayVerdictsCheckKt"
}

tasks.named<JavaExec>("run") {
    systemProperty("jna.library.path", providers.gradleProperty("library").get())
    args(providers.gradleProperty("fixture").get())
}
